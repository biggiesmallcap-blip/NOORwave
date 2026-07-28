// Home suggestions (library-resolved, cross-artist murals)
//
// Powers the Library home "Suggested tracks / albums" murals. The old client
// path expanded the seeds into "more tracks by the same artists / same albums",
// which produced clone-of-what-you-just-played suggestions. This endpoint runs
// the real radio blend per seed - the library-only sources, embedding
// neighbours and the precomputed similarity graph, since only library-resolved
// candidates can be shown here - then ranks with a per-artist cap and hub
// suppression so no single artist floods a panel. Cached 6h per seed set.

use crate::SharedState;
use axum::{extract::State, http::StatusCode, response::Json};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

use super::home_routes::{recommendation_seed_window, rotate_take, unix_now_secs};

// Seed budget. 3 recent plays keep the panel reactive to today's listening;
// 5 long-term seeds anchor it to the user's actual taste so one genre session
// cannot hijack the whole mural. Both slices are deduped against each other and
// the long-term slice rotates on the 6h salt so the panel refreshes through the
// day rather than freezing on the same five favourites.
const HOME_SUGGESTIONS_RECENT_SEEDS: usize = 3;
const HOME_SUGGESTIONS_LONG_TERM_SEEDS: usize = 5;
const HOME_SUGGESTIONS_SEED_LIMIT: usize =
    HOME_SUGGESTIONS_RECENT_SEEDS + HOME_SUGGESTIONS_LONG_TERM_SEEDS;
// Candidates pulled per seed. Only library-resolved candidates can be shown
// here, and the blend spends its budget by source weight, so this has to be
// generous: at 24 with the Last.fm source still attached, a real library gave
// back ~8 usable tracks for a panel that shows 12. Costs one bigger neighbour
// query per seed and no network at all (see `compute_home_suggestions`).
const HOME_SUGGESTIONS_PER_SEED: usize = 60;
const HOME_SUGGESTIONS_DEFAULT_LIMIT: usize = 50;
const HOME_SUGGESTIONS_MAX_PER_ARTIST: usize = 2;
// One track per album in the tracks mural, so a single unexplored record cannot
// fill the row. The album mural has its own recall path and is unaffected.
const HOME_SUGGESTIONS_MAX_PER_ALBUM: usize = 1;
// How far back a play disqualifies a track (and its album) from being suggested.
// Named so it can be tuned without restructuring: 30 days is aggressive for a
// small library but right for a large, mostly-unplayed one.
const HOME_SUGGESTIONS_RECENCY_EXCLUSION_DAYS: i64 = 30;
// How many top artists feed the long-term seed rotation.
const HOME_SUGGESTIONS_TOP_ARTIST_POOL: i64 = 20;
// Album mural asks for more than it shows so client-side dedup has slack.
const HOME_SUGGESTIONS_ALBUM_LIMIT: usize = 24;
// Albums one artist may contribute before the rest of the panel gets a turn.
// A prolific favourite otherwise fills the mural with their own back catalogue.
const HOME_SUGGESTIONS_MAX_ALBUMS_PER_ARTIST: usize = 2;
// Multiplier on the SQL LIMIT so the per-artist cap has spare candidates.
const HOME_SUGGESTIONS_ALBUM_OVERFETCH: usize = 4;
// How old a cached payload may be and still be served instantly while a fresh
// one computes behind it. Also the prune horizon, since a payload past its 6h
// freshness window is exactly what this path hands back.
const HOME_SUGGESTIONS_STALE_MAX_AGE_SECS: i64 = 7 * 24 * 60 * 60;
// Payload-shape version, embedded in every cache key. Bump it whenever the
// response shape changes so older payloads can never be served: the
// stale-while-revalidate path reads by prefix, not by exact key, and a v1
// payload (tracks only, no albums) handed to a v2 client renders an empty
// albums mural. Bump it for a materially different payload too: v3 widened the
// candidate funnel, and without a bump the stale path would keep serving the
// old short track list for up to a week.
const HOME_SUGGESTIONS_CACHE_VERSION: &str = "v3";

#[derive(Debug, serde::Deserialize)]
pub(crate) struct HomeSuggestionsRequest {
    /// Optional. When supplied these prime the "recent" seed slice; when absent
    /// the server derives it from `listen_history` itself. Either way the
    /// long-term slice comes from the user's top artists.
    #[serde(default)]
    seed_track_ids: Vec<i64>,
    limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct HomeSuggestionCandidate {
    pub(crate) track_id: i64,
    pub(crate) artist_key: String,
    pub(crate) album_id: Option<i64>,
    pub(crate) score: f64,
    pub(crate) seed_hits: u32,
    pub(crate) hub_pct: f64,
    /// Lifetime plays of this track.
    pub(crate) track_plays: i64,
    /// Lifetime plays summed across every track on this track's album.
    pub(crate) album_plays: i64,
}

/// Pure ranking pass over the aggregated library candidates. Sorts by
/// similarity, boosted when several seeds surface the same candidate
/// (consensus), dampened by how "hubby" the candidate is in the similarity
/// graph, and multiplied by two novelty terms that push never-opened albums and
/// never-played tracks to the top. Then greedily caps how many tracks any one
/// artist and any one album can contribute, and tops the list back up from what
/// the caps skipped. Returns track ids in final display order. No DB access ->
/// unit-testable.
///
/// The top-up matters: the caps are there to shape the *head* of the panel for
/// variety, not to shorten it. Library recall for a seed is same-artist-heavy,
/// so a strict cap returned 8 tracks for a mural that shows 12 - and the caller
/// has no way to tell "the library had nothing else" apart from "the cap ate
/// it". Same two-pass shape as [`cap_albums_per_artist`].
pub(crate) fn merge_home_suggestions(
    mut candidates: Vec<HomeSuggestionCandidate>,
    limit: usize,
    max_per_artist: usize,
    max_per_album: usize,
) -> Vec<i64> {
    // Album novelty dominates track novelty on purpose: an unopened record is a
    // better discovery unit than a stray unplayed track on a record the user has
    // already worn through.
    fn album_novelty(album_plays: i64) -> f64 {
        match album_plays {
            0 => 1.60,
            1..=5 => 1.15,
            _ => 0.75,
        }
    }

    fn track_novelty(track_plays: i64) -> f64 {
        match track_plays {
            0 => 1.35,
            1..=2 => 1.10,
            _ => 0.85,
        }
    }

    fn rank_of(c: &HomeSuggestionCandidate) -> f64 {
        let consensus = 1.0 + 0.15 * (c.seed_hits.saturating_sub(1) as f64);
        let hub_damp = 1.0 - 0.40 * c.hub_pct.clamp(0.0, 1.0);
        c.score.max(0.0)
            * consensus
            * hub_damp
            * album_novelty(c.album_plays)
            * track_novelty(c.track_plays)
    }

    candidates.sort_by(|a, b| {
        rank_of(b)
            .partial_cmp(&rank_of(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.track_id.cmp(&b.track_id))
    });

    let mut per_artist: HashMap<String, usize> = HashMap::new();
    let mut per_album: HashMap<i64, usize> = HashMap::new();
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for c in candidates {
        if out.len() >= limit {
            break;
        }
        // A missing album id is never capped, so those candidates don't all
        // collapse into one synthetic "album" bucket.
        let album_capped = max_per_album > 0
            && c.album_id
                .is_some_and(|id| per_album.get(&id).copied().unwrap_or(0) >= max_per_album);
        // Empty artist key (missing name) is never capped for the same reason.
        let artist_capped = max_per_artist > 0
            && !c.artist_key.is_empty()
            && per_artist.get(&c.artist_key).copied().unwrap_or(0) >= max_per_artist;
        if album_capped || artist_capped {
            skipped.push(c.track_id);
            continue;
        }
        if let Some(album_id) = c.album_id {
            *per_album.entry(album_id).or_insert(0) += 1;
        }
        if !c.artist_key.is_empty() {
            *per_artist.entry(c.artist_key).or_insert(0) += 1;
        }
        out.push(c.track_id);
    }

    // Top up in rank order from what the caps skipped, so the caps shape the
    // head of the panel without ever returning fewer tracks than exist.
    for track_id in skipped {
        if out.len() >= limit {
            break;
        }
        out.push(track_id);
    }
    out
}

/// An album card for the "Suggested albums" mural. Carries exactly the fields
/// the frontend card needs, so the client no longer reconstructs albums from
/// whichever loose tracks happened to survive track ranking.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct SuggestedAlbum {
    pub(crate) id: i64,
    pub(crate) title: String,
    pub(crate) artist_id: Option<i64>,
    pub(crate) artist_name: Option<String>,
    pub(crate) artwork_url: Option<String>,
}

/// Distinct artist ids for the given tracks.
fn track_artist_ids(conn: &rusqlite::Connection, track_ids: &[i64]) -> anyhow::Result<Vec<i64>> {
    if track_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = vec!["?"; track_ids.len()].join(",");
    let sql = format!(
        "SELECT DISTINCT artist_id FROM tracks WHERE id IN ({placeholders}) AND artist_id IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let ids = stmt
        .query_map(rusqlite::params_from_iter(track_ids.iter()), |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// `track_id -> album_id` for the candidate set.
fn candidate_album_ids(
    conn: &rusqlite::Connection,
    track_ids: &[i64],
) -> anyhow::Result<HashMap<i64, Option<i64>>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; track_ids.len()].join(",");
    let sql = format!("SELECT id, album_id FROM tracks WHERE id IN ({placeholders})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(track_ids.iter()), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
        })?
        .collect::<Result<HashMap<_, _>, _>>()?;
    Ok(rows)
}

/// `track_id -> (track_plays, album_plays)` for the candidate set. `album_plays`
/// is the lifetime play total across every track on that track's album, and is
/// 0 for tracks with no album. One query for the whole set; the ranking function
/// stays pure.
fn candidate_play_stats(
    conn: &rusqlite::Connection,
    track_ids: &[i64],
) -> anyhow::Result<HashMap<i64, (i64, i64)>> {
    if track_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = vec!["?"; track_ids.len()].join(",");
    let sql = format!(
        "SELECT t.id,
                COALESCE(t.play_count, 0),
                COALESCE((
                    SELECT SUM(COALESCE(sib.play_count, 0))
                    FROM tracks sib
                    WHERE sib.album_id = t.album_id
                ), 0)
         FROM tracks t
         WHERE t.id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(track_ids.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
            ))
        })?
        .collect::<Result<HashMap<_, _>, _>>()?;
    Ok(rows)
}

/// Albums by `artist_ids` where not one track has ever been played, ranked by
/// how strongly their tracks connect to `seed_track_ids` in the precomputed
/// `track_similarity` graph. This is the album mural's own recall path: the
/// user has thousands of never-opened albums, and reconstructing album cards
/// from track-level ranking surfaced almost none of them.
///
/// Albums with no similarity edges to the seeds at all still qualify (they sort
/// last, by album id) - an unopened record by a favourite artist is a
/// legitimate suggestion even when the graph has nothing to say about it.
///
/// A per-artist cap is applied after ranking. Without it a prolific favourite
/// buries everything else: on a real library the raw top 24 came back with 9
/// Bob Marley records and 4 Howard Shore soundtracks, which is a discography
/// listing rather than a discovery panel. The query over-fetches so the cap has
/// material to fall through to.
fn unexplored_albums_for_artists(
    conn: &rusqlite::Connection,
    artist_ids: &[i64],
    seed_track_ids: &[i64],
    limit: usize,
) -> anyhow::Result<Vec<SuggestedAlbum>> {
    if artist_ids.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let artist_ph = vec!["?"; artist_ids.len()].join(",");
    // An empty seed list would make `IN ()` invalid, so fall back to an id that
    // never matches; every album then scores 0 and sorts by id.
    let seed_ph = if seed_track_ids.is_empty() {
        "-1".to_string()
    } else {
        vec!["?"; seed_track_ids.len()].join(",")
    };

    let sql = format!(
        "WITH zero_play_album AS (
             SELECT t.album_id AS album_id
             FROM tracks t
             WHERE t.album_id IS NOT NULL AND t.artist_id IN ({artist_ph})
             GROUP BY t.album_id
             HAVING SUM(COALESCE(t.play_count, 0)) = 0
         ),
         scored AS (
             SELECT zpa.album_id AS album_id,
                    COALESCE(SUM(ts.similarity_score), 0.0) AS sim_total
             FROM zero_play_album zpa
             JOIN tracks t2 ON t2.album_id = zpa.album_id
             LEFT JOIN track_similarity ts
                 ON (ts.track_a = t2.id AND ts.track_b IN ({seed_ph}))
                 OR (ts.track_b = t2.id AND ts.track_a IN ({seed_ph}))
             GROUP BY zpa.album_id
         )
         SELECT al.id, al.title, al.artist_id, ar.name, al.artwork_url
         FROM scored s
         JOIN albums al ON al.id = s.album_id
         LEFT JOIN artists ar ON ar.id = al.artist_id
         ORDER BY s.sim_total DESC, al.id ASC
         LIMIT ?"
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut bound: Vec<i64> = Vec::new();
    bound.extend_from_slice(artist_ids);
    if !seed_track_ids.is_empty() {
        // The seed list appears twice in the LEFT JOIN predicate.
        bound.extend_from_slice(seed_track_ids);
        bound.extend_from_slice(seed_track_ids);
    }
    // Over-fetch so the per-artist cap below has lower-ranked albums by other
    // artists to fall through to instead of just shortening the list.
    bound.push((limit * HOME_SUGGESTIONS_ALBUM_OVERFETCH) as i64);

    let rows = stmt
        .query_map(rusqlite::params_from_iter(bound.iter()), |row| {
            Ok(SuggestedAlbum {
                id: row.get(0)?,
                title: row.get(1)?,
                artist_id: row.get(2)?,
                artist_name: row.get(3)?,
                artwork_url: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(cap_albums_per_artist(
        rows,
        limit,
        HOME_SUGGESTIONS_MAX_ALBUMS_PER_ARTIST,
    ))
}

/// Greedy per-artist cap over ranked albums, with a second pass that tops the
/// list back up from what the cap skipped. The cap shapes the head of the panel
/// for variety without ever returning fewer albums than it could have. A missing
/// artist id is never capped, mirroring the track-side rules. No DB access ->
/// unit-testable.
pub(crate) fn cap_albums_per_artist(
    albums: Vec<SuggestedAlbum>,
    limit: usize,
    max_per_artist: usize,
) -> Vec<SuggestedAlbum> {
    let mut per_artist: HashMap<i64, usize> = HashMap::new();
    let mut chosen: Vec<SuggestedAlbum> = Vec::new();
    let mut skipped: Vec<SuggestedAlbum> = Vec::new();

    for album in albums {
        if chosen.len() >= limit {
            break;
        }
        if max_per_artist > 0 {
            if let Some(artist_id) = album.artist_id {
                let count = per_artist.entry(artist_id).or_insert(0);
                if *count >= max_per_artist {
                    skipped.push(album);
                    continue;
                }
                *count += 1;
            }
        }
        chosen.push(album);
    }

    for album in skipped {
        if chosen.len() >= limit {
            break;
        }
        chosen.push(album);
    }

    chosen
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

/// Shared prefix for every cache key of the current payload shape. The stale
/// path matches on this so it can never resurrect an older shape.
fn home_suggestions_cache_key_prefix() -> String {
    format!("home_suggest:{HOME_SUGGESTIONS_CACHE_VERSION}:")
}

fn home_suggestions_cache_key(seed_ids: &[i64], limit: usize) -> String {
    let mut sorted = seed_ids.to_vec();
    sorted.sort_unstable();
    let joined = sorted
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join("-");
    format!("{}{limit}:{joined}", home_suggestions_cache_key_prefix())
}

fn read_home_suggestions_cache(db: &crate::db::Database, cache_key: &str) -> Option<Value> {
    let now = unix_now_secs();
    let key = cache_key.to_string();
    db.with_conn(|conn| {
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

/// The most recently computed payload under ANY seed set, ignoring expiry but
/// bounded by `max_age_secs`. Backs the stale-while-revalidate path: the exact
/// seed set changes every time the user plays something (the recent slice moves),
/// so an exact-key miss is the common case on a boot after listening. Serving the
/// last good payload instantly and refreshing behind it keeps the murals off the
/// critical path instead of paying the fan-out in the foreground.
fn read_recent_home_suggestions_cache(
    db: &crate::db::Database,
    max_age_secs: i64,
) -> Option<Value> {
    let floor = unix_now_secs() - max_age_secs.max(0);
    // Scoped to the current payload version. Matching on provider alone would
    // resurrect payloads written by an older build whose shape the client can no
    // longer render.
    let prefix = format!("{}%", home_suggestions_cache_key_prefix());
    db.with_conn(|conn| {
        conn.query_row(
            "SELECT payload_json FROM provider_recommendation_cache
                  WHERE provider = 'home_suggestions'
                    AND cache_key LIKE ?1
                    AND fetched_at >= ?2
                  ORDER BY fetched_at DESC
                  LIMIT 1",
            params![prefix, floor],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(Into::into)
    })
    .ok()
    .flatten()
    .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn write_home_suggestions_cache(db: &crate::db::Database, cache_key: &str, payload: &Value) {
    let now = unix_now_secs();
    let expires = now + 6 * 60 * 60;
    let Ok(serialized) = serde_json::to_string(payload) else {
        return;
    };
    let key = cache_key.to_string();
    let _ = db.with_conn(|conn| {
        // Prune old rows so the per-seed-set keys don't accumulate unbounded.
        // Prunes by age, not by expiry: an expired row is still the payload the
        // stale-while-revalidate path serves for instant first paint, so it has
        // to outlive its own freshness window.
        let _ = conn.execute(
            "DELETE FROM provider_recommendation_cache
                  WHERE provider = 'home_suggestions' AND fetched_at < ?1",
            params![now - HOME_SUGGESTIONS_STALE_MAX_AGE_SECS],
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

/// POST /api/home/suggestions - hidden-gem picks for the Library home murals.
/// Body: `{ seed_track_ids?, limit? }`. Returns `{ tracks, albums }`.
///
/// Stale-while-revalidate. The seed set embeds the user's 3 most recent plays,
/// so listening to anything moves the cache key and an exact hit is the
/// exception, not the rule, on a boot after a listening session. Rather than pay
/// the multi-second fan-out in the foreground every time, an exact miss serves
/// the last good payload immediately and recomputes behind it. Only a user who
/// has never loaded the murals waits.
pub(crate) async fn get_home_suggestions(
    State(state): State<SharedState>,
    Json(req): Json<HomeSuggestionsRequest>,
) -> Result<Json<Value>, StatusCode> {
    let limit = req
        .limit
        .unwrap_or(HOME_SUGGESTIONS_DEFAULT_LIMIT)
        .clamp(1, 60);

    let db = {
        let g = state.read().await;
        g.db.clone()
    };

    // Seeds: the client's recent list if it sent one, otherwise our own, blended
    // with long-term top artists and rotated on the 6h window.
    let salt = recommendation_seed_window();
    let client_recent: Vec<i64> = req
        .seed_track_ids
        .iter()
        .copied()
        .filter(|&id| id > 0)
        .collect();
    let seeds = db
        .with_conn(move |conn| {
            let recent = if client_recent.is_empty() {
                recent_seed_pool(conn, HOME_SUGGESTIONS_RECENT_SEEDS as i64 * 4)?
            } else {
                client_recent
            };
            let long_term = long_term_seed_pool(conn, HOME_SUGGESTIONS_TOP_ARTIST_POOL)?;
            Ok(blend_suggestion_seeds(&recent, &long_term, salt))
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if seeds.is_empty() {
        return Ok(Json(json!({ "tracks": [], "albums": [] })));
    }

    let cache_key = home_suggestions_cache_key(&seeds, limit);
    if let Some(cached) = read_home_suggestions_cache(&db, &cache_key) {
        return Ok(Json(cached));
    }

    // Exact miss. If anything recent is on disk, hand it back now and refresh in
    // the background. Duplicate spawns are possible if two loads race, but the
    // client fetches once per mount per refresh bucket and the write is
    // idempotent, so the cost is a wasted recompute rather than a wrong answer.
    if let Some(stale) =
        read_recent_home_suggestions_cache(&db, HOME_SUGGESTIONS_STALE_MAX_AGE_SECS)
    {
        let bg_state = state.clone();
        tokio::spawn(async move {
            match compute_home_suggestions(&bg_state, &seeds, limit).await {
                Ok(payload) => {
                    let bg_db = {
                        let g = bg_state.read().await;
                        g.db.clone()
                    };
                    write_home_suggestions_cache(&bg_db, &cache_key, &payload);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "home_suggestions: background refresh failed");
                }
            }
        });
        return Ok(Json(stale));
    }

    // Nothing cached at all (first ever load): compute in the foreground.
    let payload = compute_home_suggestions(&state, &seeds, limit)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    write_home_suggestions_cache(&db, &cache_key, &payload);
    Ok(Json(payload))
}

/// The actual fan-out + ranking. Split out of the handler so the
/// stale-while-revalidate path can run it in a background task.
async fn compute_home_suggestions(
    state: &SharedState,
    seeds: &[i64],
    limit: usize,
) -> anyhow::Result<Value> {
    let db = {
        let g = state.read().await;
        g.db.clone()
    };
    let seeds = seeds.to_vec();

    let exclusions = db
        .with_conn(|conn| recent_play_exclusions(conn, HOME_SUGGESTIONS_RECENCY_EXCLUSION_DAYS))
        .unwrap_or_default();

    // Aggregate library-resolved candidates across seeds. Dedup by track id,
    // count how many seeds surfaced each (consensus) and keep the best score.
    //
    // The Last.fm source is deliberately switched off (`None`). Its candidates
    // are always `is_in_library: false` with `track_id: 0` - they are external
    // matches the radio queue resolves lazily at play time - so this endpoint,
    // which keeps only library-resolved rows, discarded every one of them while
    // they still consumed most of the per-seed budget (20 of 24 slots on a real
    // library). Dropping them is not a recall loss, and it takes the whole
    // fan-out off the network, which is what made the cold path multi-second.
    let queues = futures::future::join_all(seeds.iter().map(|&seed_id| {
        let db = &db;
        let seeds_ref = &seeds;
        async move {
            crate::services::radio::orchestrate_song(
                db,
                None,
                None,
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
            if !cand.is_in_library
                || seeds.contains(&cand.track_id)
                || exclusions.track_ids.contains(&cand.track_id)
            {
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
                    album_id: None,
                    score: cand.similarity_score,
                    seed_hits: 1,
                    hub_pct,
                    track_plays: 0,
                    album_plays: 0,
                });
        }
    }

    // Hydrate album ids + play totals, drop anything on a recently played album,
    // then rank.
    let candidate_ids: Vec<i64> = agg.keys().copied().collect();
    let ids_for_stats = candidate_ids.clone();
    let (album_by_track, play_stats) = db.with_conn(move |conn| {
        let albums = candidate_album_ids(conn, &ids_for_stats)?;
        let stats = candidate_play_stats(conn, &ids_for_stats)?;
        Ok((albums, stats))
    })?;

    let mut candidates: Vec<HomeSuggestionCandidate> = Vec::new();
    for (track_id, mut cand) in agg {
        let album_id = album_by_track.get(&track_id).copied().flatten();
        if album_id.is_some_and(|id| exclusions.album_ids.contains(&id)) {
            continue;
        }
        let (track_plays, album_plays) = play_stats.get(&track_id).copied().unwrap_or((0, 0));
        cand.album_id = album_id;
        cand.track_plays = track_plays;
        cand.album_plays = album_plays;
        candidates.push(cand);
    }

    let ranked_ids = merge_home_suggestions(
        candidates,
        limit,
        HOME_SUGGESTIONS_MAX_PER_ARTIST,
        HOME_SUGGESTIONS_MAX_PER_ALBUM,
    );

    // Hydrate to full Track rows, then restore rank order (get_tracks_by_ids
    // returns rows in arbitrary order).
    let ids_for_query = ranked_ids.clone();
    let tracks =
        db.with_conn(move |conn| crate::playback::queue::get_tracks_by_ids(conn, &ids_for_query))?;
    let mut by_id: HashMap<i64, crate::db::models::Track> =
        tracks.into_iter().map(|t| (t.id, t)).collect();
    let ordered: Vec<crate::db::models::Track> = ranked_ids
        .iter()
        .filter_map(|id| by_id.remove(id))
        .collect();

    // The album mural gets its own recall path: never-opened albums by artists
    // the user already listens to, ranked by similarity to the seeds. The artist
    // pool is the seed artists themselves plus the artists that survived track
    // ranking. Seed artists must be looked up from the DB, not from `by_id` -
    // seeds are excluded from candidacy, so they were never in that map.
    let seeds_for_artists = seeds.clone();
    let seed_artist_ids: Vec<i64> = db
        .with_conn(move |conn| track_artist_ids(conn, &seeds_for_artists))
        .unwrap_or_default()
        .into_iter()
        .chain(ordered.iter().map(|t| t.artist_id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let seeds_for_albums = seeds.clone();
    let excluded_albums = exclusions.album_ids.clone();
    let albums: Vec<SuggestedAlbum> = db
        .with_conn(move |conn| {
            unexplored_albums_for_artists(
                conn,
                &seed_artist_ids,
                &seeds_for_albums,
                HOME_SUGGESTIONS_ALBUM_LIMIT,
            )
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|a| !excluded_albums.contains(&a.id))
        .collect();

    Ok(json!({ "tracks": ordered, "albums": albums }))
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

    fn album(id: i64, artist_id: Option<i64>) -> SuggestedAlbum {
        SuggestedAlbum {
            id,
            title: format!("Album {id}"),
            artist_id,
            artist_name: artist_id.map(|a| format!("Artist {a}")),
            artwork_url: None,
        }
    }

    #[test]
    fn album_cap_keeps_a_prolific_favourite_from_owning_the_panel() {
        // Verified against the real library: the raw ranking put 9 Bob Marley
        // records in the top 24. The cap must let other artists through first.
        let ranked = vec![
            album(1, Some(10)),
            album(2, Some(10)),
            album(3, Some(10)),
            album(4, Some(10)),
            album(5, Some(20)),
            album(6, Some(30)),
        ];
        let capped = cap_albums_per_artist(ranked, 4, 2);
        let ids: Vec<i64> = capped.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![1, 2, 5, 6]);
    }

    #[test]
    fn album_cap_backfills_rather_than_returning_a_short_list() {
        // Only one artist has unopened albums. Shortening the panel would be
        // worse than showing more of the one artist available.
        let ranked = (1..=5).map(|id| album(id, Some(10))).collect();
        let capped = cap_albums_per_artist(ranked, 4, 2);
        let ids: Vec<i64> = capped.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn album_cap_never_caps_a_missing_artist() {
        let ranked = (1..=3).map(|id| album(id, None)).collect();
        let capped = cap_albums_per_artist(ranked, 3, 2);
        assert_eq!(capped.len(), 3);
    }

    #[test]
    fn stale_read_returns_newest_payload_under_any_seed_set() {
        let db = fresh_migrated_db();
        // Two payloads under different seed sets. The newer one is already past
        // its freshness window, which is exactly the case the stale path exists
        // for: the user played something, so no exact key will ever hit.
        write_home_suggestions_cache(
            &db,
            &home_suggestions_cache_key(&[1, 2, 3], 50),
            &json!({"tracks":["old"]}),
        );
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write_home_suggestions_cache(
            &db,
            &home_suggestions_cache_key(&[4, 5, 6], 50),
            &json!({"tracks":["new"]}),
        );
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE provider_recommendation_cache SET expires_at = 1
                     WHERE provider = 'home_suggestions'",
                [],
            )?;
            Ok(())
        })
        .expect("expire rows");

        // An exact hit must still respect expiry.
        assert!(
            read_home_suggestions_cache(&db, &home_suggestions_cache_key(&[4, 5, 6], 50)).is_none(),
            "expired rows are not fresh hits"
        );
        // The stale path ignores expiry and takes the most recently written.
        let stale = read_recent_home_suggestions_cache(&db, 7 * 24 * 60 * 60)
            .expect("a stale payload is available");
        assert_eq!(stale["tracks"][0], "new");
    }

    #[test]
    fn stale_read_never_resurrects_an_older_payload_shape() {
        // Regression: the stale path originally matched on provider alone, so a
        // v1 payload (tracks only, no albums) was served to a v2 client and the
        // albums mural rendered empty. Caught end-to-end against a real library.
        let db = fresh_migrated_db();
        write_home_suggestions_cache(&db, "home_suggest:v1:50:1-2-3", &json!({"tracks":["old"]}));

        assert!(
            read_recent_home_suggestions_cache(&db, 7 * 24 * 60 * 60).is_none(),
            "a payload from an older cache version must not be served"
        );

        write_home_suggestions_cache(
            &db,
            &home_suggestions_cache_key(&[4, 5, 6], 50),
            &json!({"tracks":["new"],"albums":[]}),
        );
        let stale = read_recent_home_suggestions_cache(&db, 7 * 24 * 60 * 60)
            .expect("current-version payload is served");
        assert_eq!(stale["tracks"][0], "new");
    }

    #[test]
    fn stale_read_ignores_payloads_older_than_the_window() {
        let db = fresh_migrated_db();
        write_home_suggestions_cache(
            &db,
            &home_suggestions_cache_key(&[1], 50),
            &json!({"tracks":["ancient"]}),
        );
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE provider_recommendation_cache SET fetched_at = 1
                     WHERE provider = 'home_suggestions'",
                [],
            )?;
            Ok(())
        })
        .expect("age rows");

        assert!(
            read_recent_home_suggestions_cache(&db, 7 * 24 * 60 * 60).is_none(),
            "a payload older than the window is not served"
        );
    }

    #[test]
    fn cache_key_is_versioned_and_seed_order_independent() {
        let a = home_suggestions_cache_key(&[3, 1, 2], 50);
        let b = home_suggestions_cache_key(&[1, 2, 3], 50);
        assert_eq!(a, b, "seed order must not fragment the cache");
        assert!(
            a.starts_with(&home_suggestions_cache_key_prefix()),
            "shape change requires a version bump, got {a}"
        );
        assert_ne!(a, home_suggestions_cache_key(&[1, 2, 3], 20));
    }

    #[test]
    fn play_stats_report_track_and_album_totals() {
        let db = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'A')", [])?;
            conn.execute(
                "INSERT INTO albums (id, title, artist_id) VALUES (10, 'Worn', 1), (11, 'Sealed', 1)",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, album_id, file_path, play_count) VALUES
                    (100, 'Hit', 1, 10, '/1', 7),
                    (101, 'Deep Cut', 1, 10, '/2', 0),
                    (102, 'Untouched', 1, 11, '/3', 0),
                    (103, 'Loose', 1, NULL, '/4', 2)",
                [],
            )?;
            Ok(())
        })
        .expect("seed fixture");

        let stats = db
            .with_conn(|conn| candidate_play_stats(conn, &[100, 101, 102, 103]))
            .expect("stats");

        assert_eq!(stats.get(&100), Some(&(7, 7)), "track plays, album plays");
        assert_eq!(
            stats.get(&101),
            Some(&(0, 7)),
            "unplayed track on a worn album still carries the album total"
        );
        assert_eq!(stats.get(&102), Some(&(0, 0)));
        assert_eq!(
            stats.get(&103),
            Some(&(2, 0)),
            "album total is zero when the track has no album"
        );
    }

    #[test]
    fn unexplored_albums_prefer_strong_similarity_and_skip_played_records() {
        let db = fresh_migrated_db();
        db.with_conn(|conn| {
            conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Loved')", [])?;
            conn.execute(
                "INSERT INTO albums (id, title, artist_id, artwork_url) VALUES
                    (10, 'Seed Home', 1, NULL),
                    (11, 'Close Neighbour', 1, 'art11'),
                    (12, 'Distant Neighbour', 1, 'art12')",
                [],
            )?;
            conn.execute(
                "INSERT INTO tracks (id, title, artist_id, album_id, file_path, play_count) VALUES
                    (100, 'Seed', 1, 10, '/s', 5),
                    (110, 'Close', 1, 11, '/c', 0),
                    (120, 'Distant', 1, 12, '/d', 0)",
                [],
            )?;
            // track_similarity enforces track_a < track_b.
            conn.execute(
                "INSERT INTO track_similarity (track_a, track_b, similarity_score) VALUES
                    (100, 110, 0.9),
                    (100, 120, 0.2)",
                [],
            )?;
            Ok(())
        })
        .expect("seed fixture");

        let albums = db
            .with_conn(|conn| unexplored_albums_for_artists(conn, &[1], &[100], 10))
            .expect("albums");

        let ids: Vec<i64> = albums.iter().map(|a| a.id).collect();
        assert_eq!(
            ids,
            vec![11, 12],
            "zero-play albums only, strongest similarity first"
        );
        assert_eq!(albums[0].artwork_url.as_deref(), Some("art11"));
        assert_eq!(albums[0].artist_name.as_deref(), Some("Loved"));
    }

    /// Each candidate lands on its own album by default so the per-album cap
    /// stays out of the way of tests that are about artist capping or ordering.
    fn home_cand(track_id: i64, artist: &str, score: f64) -> HomeSuggestionCandidate {
        HomeSuggestionCandidate {
            track_id,
            artist_key: artist.to_string(),
            album_id: Some(track_id),
            score,
            seed_hits: 1,
            hub_pct: 0.0,
            track_plays: 0,
            album_plays: 0,
        }
    }

    #[test]
    fn unopened_album_outranks_a_worn_one_at_equal_similarity() {
        let unopened = home_cand(1, "a", 0.5);
        let worn = HomeSuggestionCandidate {
            album_plays: 40,
            ..home_cand(2, "b", 0.5)
        };
        let ranked = merge_home_suggestions(vec![worn, unopened], 2, 2, 1);
        assert_eq!(ranked, vec![1, 2]);
    }

    #[test]
    fn never_played_track_outranks_a_familiar_one() {
        let fresh = HomeSuggestionCandidate {
            album_plays: 3,
            ..home_cand(1, "a", 0.5)
        };
        let familiar = HomeSuggestionCandidate {
            album_plays: 3,
            track_plays: 9,
            ..home_cand(2, "b", 0.5)
        };
        let ranked = merge_home_suggestions(vec![familiar, fresh], 2, 2, 1);
        assert_eq!(ranked, vec![1, 2]);
    }

    #[test]
    fn album_cap_stops_one_record_filling_the_row() {
        // Three tracks off the same unopened album hold the top scores. With a
        // cap of 1 per album the best one leads and the other album's weaker
        // track takes the second slot, so the head of the panel is not one
        // record. The remaining slot is then topped up from what the cap
        // skipped rather than left short.
        let cands = vec![
            HomeSuggestionCandidate {
                album_id: Some(10),
                ..home_cand(1, "same", 0.99)
            },
            HomeSuggestionCandidate {
                album_id: Some(10),
                ..home_cand(2, "same", 0.98)
            },
            HomeSuggestionCandidate {
                album_id: Some(10),
                ..home_cand(3, "same", 0.97)
            },
            HomeSuggestionCandidate {
                album_id: Some(11),
                ..home_cand(4, "other", 0.10)
            },
        ];
        let ranked = merge_home_suggestions(cands, 3, 2, 1);
        assert_eq!(ranked, vec![1, 4, 2]);
    }

    #[test]
    fn caps_top_up_instead_of_returning_a_short_panel() {
        // Everything is by one artist on one album: the caps have nothing to
        // fall through to, so a strict cap would hand back a single track for a
        // panel that shows four. The caps shape the head; they must not shorten
        // the list below what the library actually offered.
        let cands = (1..=6)
            .map(|id| HomeSuggestionCandidate {
                album_id: Some(10),
                ..home_cand(id, "solo", 1.0 - id as f64 * 0.01)
            })
            .collect();
        let ranked = merge_home_suggestions(cands, 4, 2, 1);
        assert_eq!(ranked, vec![1, 2, 3, 4]);
    }

    #[test]
    fn candidates_without_an_album_are_never_album_capped() {
        // A missing album_id must not collapse every such track into one
        // synthetic bucket, mirroring the empty-artist-key rule.
        let cands = (1..=3)
            .map(|id| HomeSuggestionCandidate {
                album_id: None,
                ..home_cand(id, &format!("artist{id}"), 0.5)
            })
            .collect();
        let ranked = merge_home_suggestions(cands, 3, 2, 1);
        assert_eq!(ranked, vec![1, 2, 3]);
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
            1,
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
            1,
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
        let ranked = merge_home_suggestions(vec![single, hub, consensus], 3, 2, 1);
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
            1,
        );
        assert_eq!(ranked, vec![1, 2, 3]);
    }
}
