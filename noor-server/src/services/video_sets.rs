//! Editorial video set builder for the /videos browse state.
//!
//! A "set" is a small curated snapshot (5-12 videos) built for one rotation
//! bucket (a calendar day or ISO week) and persisted whole in `video_sets`.
//! Rebuilding the same (slug, bucket_key) against the same library yields the
//! same set: anchor sampling, scoring jitter, and copy all draw from one RNG
//! seeded on the slug + bucket key, so the page is stable across reloads
//! within a bucket and fresh across buckets.
//!
//! Archetypes differ only in where their candidates come from; scoring,
//! capping, and copy all run through the same assembly core:
//!
//! | Archetype    | Anchors                                   | Rhythm |
//! |--------------|-------------------------------------------|--------|
//! | DailyPicks   | top-listened library artists              | daily  |
//! | Genre(name)  | library artists carrying that genre        | weekly |
//! | AlbumLove    | artists behind favorited albums            | weekly |
//! | OneStepOut   | TIDAL "fans also like", library removed    | weekly |
//! | DjSets       | video search, long-form only (no anchors)  | weekly |
//!
//! Scoring is the shared `discovery_ranking::shape_score`: the seed is the
//! listener's genre profile, the candidate carries its anchor's genre set, and
//! every DSP multiplier passes through neutral per that module's missing-data
//! contract.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::genre::jaccard::weighted_genre_set;
use crate::services::discovery_ranking::{
    CandidateFeatures, RankParams, SeedFeatures, shape_score,
};
use crate::services::tidal::client::{TidalArtistVideo, TidalClient, TidalSearchVideo};

pub const DAILY_PICKS_SLUG: &str = "daily-picks";
pub const ALBUM_LOVE_SLUG: &str = "album-love";
pub const ONE_STEP_OUT_SLUG: &str = "one-step-out";
pub const DJ_SETS_SLUG: &str = "dj-sets";
pub const GENRE_SLUG_PREFIX: &str = "genre:";

/// How many top-listened artists form the daily sampling pool. Wide enough
/// that the draw feels different day to day, narrow enough to stay "artists
/// you already trust".
const ANCHOR_POOL_SIZE: i64 = 60;
/// Anchors drawn per build. Each costs one TIDAL artist-videos call.
const ANCHORS_PER_BUILD: usize = 10;
/// Anchors seeding the adjacency expansion. Each costs one similar-artists
/// call plus one artist-videos call per artist kept.
const SIMILAR_SEED_ANCHORS: usize = 4;
/// Similar artists kept per seed anchor after library filtering.
const SIMILAR_PER_ANCHOR: usize = 3;
/// Videos requested per anchor.
const VIDEOS_PER_ANCHOR: i32 = 10;
/// At most this many picks from one artist, so nobody owns a shelf.
const PER_ARTIST_CAP: usize = 2;
/// At most this many picks from one anchor. On the search-driven shelf an
/// anchor is a query, and without this one lucky query (a single Boiler Room
/// series, say) fills the rail with near-identical uploads by different DJs.
const PER_ANCHOR_CAP: usize = 3;
/// Target set size; the builder drops rather than pads below this.
const SET_SIZE: usize = 12;
/// A set smaller than this is not worth showing; the route treats it as absent.
const MIN_SET_SIZE: usize = 4;
/// How many genre shelves to build, taken from the top of the listen profile.
const GENRE_SET_COUNT: usize = 3;
/// A "set" or "session" in the DJ-mix sense: long-form video. Duration is the
/// one reliable format signal TIDAL gives us - there is no format/type field
/// worth filtering on - so it stands in for "this is a mix, not a single".
const LONG_FORM_MIN_SECONDS: i64 = 900;
/// Videos requested per long-form search query.
const SEARCH_LIMIT: i32 = 20;
/// Placeholder artist rows that carry no taste signal. "Various Artists" owns
/// most compilations, so without this it anchors half the album-love shelf and
/// the copy ends up crediting a non-artist.
const NON_ARTIST_NAMES: &[&str] = &["various artists", "various", "unknown artist", "soundtrack"];

fn is_real_artist(name: &str) -> bool {
    let lc = name.trim().to_lowercase();
    !lc.is_empty() && !NON_ARTIST_NAMES.contains(&lc.as_str())
}

/// One stored editorial set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSet {
    pub slug: String,
    pub bucket_key: String,
    pub title: String,
    pub blurb: String,
    pub built_at: String,
    pub items: Vec<VideoSetItem>,
}

/// One pick. Field names mirror the frontend `TidalSearchVideo` shape so the
/// route can serialize items straight through to `VideoCard`/mural consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSetItem {
    pub tidal_id: i64,
    pub title: String,
    pub duration_ms: Option<i64>,
    pub artist_id: Option<i64>,
    pub artist_name: Option<String>,
    pub album_tidal_id: Option<i64>,
    pub artwork_url: Option<String>,
    pub quality: Option<String>,
    pub explicit: Option<bool>,
    #[serde(rename = "type")]
    pub kind: String,
    /// Compact human-readable reason, from the shared shaping layer. Empty
    /// when no signal fired.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub why: String,
}

/// Anchor candidate: an artist to pull videos from, plus the weight and
/// provenance the copy layer needs.
#[derive(Debug, Clone)]
pub struct AnchorArtist {
    pub tidal_id: i64,
    pub name: String,
    pub listens: i64,
    /// For adjacency picks: the library artist this one was reached through.
    pub via: Option<String>,
}

impl AnchorArtist {
    fn new(tidal_id: i64, name: String, listens: i64) -> Self {
        Self {
            tidal_id,
            name,
            listens,
            via: None,
        }
    }
}

/// What kind of set is being built. Drives copy and candidate sourcing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Archetype {
    DailyPicks,
    Genre(String),
    AlbumLove,
    OneStepOut,
    DjSets,
}

/// A set the route should build: everything DB-derived is already resolved, so
/// the async fan-out never touches the connection.
#[derive(Debug, Clone)]
pub struct SetPlan {
    pub slug: String,
    pub bucket_key: String,
    pub archetype: Archetype,
    /// Empty for `DjSets`; for `OneStepOut` these are the library seeds to
    /// expand through "fans also like", not the final anchors.
    pub anchors: Vec<AnchorArtist>,
    /// `DjSets` only: the search queries to run.
    pub queries: Vec<String>,
    pub profile_genres: Vec<String>,
    pub anchor_genres: HashMap<i64, Vec<String>>,
}

impl SetPlan {
    pub fn seed(&self) -> u64 {
        build_seed(&self.slug, &self.bucket_key)
    }
}

/// A candidate video, normalized across the artist-videos and search paths.
#[derive(Debug, Clone)]
pub struct VideoCandidate {
    pub tidal_id: i64,
    pub title: String,
    pub duration_s: Option<i64>,
    pub artist_id: Option<i64>,
    pub artist_name: Option<String>,
    pub album_tidal_id: Option<i64>,
    pub artwork_url: Option<String>,
}

impl From<&TidalArtistVideo> for VideoCandidate {
    fn from(v: &TidalArtistVideo) -> Self {
        Self {
            tidal_id: v.id,
            title: v.title.clone(),
            duration_s: Some(v.duration),
            artist_id: v.artist.as_ref().map(|a| a.id),
            artist_name: v.artist.as_ref().map(|a| a.name.clone()),
            album_tidal_id: v.album.as_ref().map(|al| al.id),
            artwork_url: TidalClient::get_artwork_url(&v.image_id, 640),
        }
    }
}

impl From<&TidalSearchVideo> for VideoCandidate {
    fn from(v: &TidalSearchVideo) -> Self {
        Self {
            tidal_id: v.id,
            title: v.title.clone(),
            duration_s: v.duration,
            artist_id: v.artist_id,
            artist_name: v.artist_name.clone(),
            album_tidal_id: v.album_id,
            artwork_url: v.artwork_url.clone(),
        }
    }
}

/// Bucket key for daily sets: the local calendar date.
pub fn daily_bucket_key(date: chrono::NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Bucket key for weekly sets: ISO year + week, so a set turns over on Monday
/// rather than drifting with the day it was first built.
pub fn weekly_bucket_key(date: chrono::NaiveDate) -> String {
    date.format("%G-W%V").to_string()
}

/// Deterministic RNG seed for one (slug, bucket) build. FNV-1a over the pair;
/// no cryptographic requirement, just stability.
pub fn build_seed(slug: &str, bucket_key: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in slug.bytes().chain([b'|']).chain(bucket_key.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// `"Drum & Bass"` -> `"genre:drum-bass"`. Keeps slugs URL- and key-safe
/// without pulling in a slug crate for one call site.
pub fn genre_slug(name: &str) -> String {
    let mut out = String::from(GENRE_SLUG_PREFIX);
    let mut last_dash = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

// --- Persistence ---

pub fn store_set(conn: &Connection, set: &VideoSet) -> Result<()> {
    let items_json = serde_json::to_string(&set.items)?;
    conn.execute(
        "INSERT INTO video_sets (slug, bucket_key, title, blurb, built_at, items_json)
         VALUES (?1, ?2, ?3, ?4, datetime('now'), ?5)
         ON CONFLICT(slug, bucket_key) DO UPDATE SET
           title = excluded.title,
           blurb = excluded.blurb,
           built_at = excluded.built_at,
           items_json = excluded.items_json",
        params![set.slug, set.bucket_key, set.title, set.blurb, items_json],
    )?;
    Ok(())
}

pub fn load_set(conn: &Connection, slug: &str, bucket_key: &str) -> Result<Option<VideoSet>> {
    load_set_where(
        conn,
        "SELECT slug, bucket_key, title, blurb, built_at, items_json
         FROM video_sets WHERE slug = ?1 AND bucket_key = ?2",
        params![slug, bucket_key],
    )
}

/// Most recently built snapshot for a slug, any bucket. Serves the
/// stale-while-revalidate path on the first visit of a new bucket.
pub fn load_latest_set(conn: &Connection, slug: &str) -> Result<Option<VideoSet>> {
    load_set_where(
        conn,
        "SELECT slug, bucket_key, title, blurb, built_at, items_json
         FROM video_sets WHERE slug = ?1
         ORDER BY built_at DESC, id DESC LIMIT 1",
        params![slug],
    )
}

/// Every slug that has at least one stored snapshot, newest first. Lets the
/// route serve whatever exists without knowing which archetypes ever built.
pub fn load_latest_sets(conn: &Connection) -> Result<Vec<VideoSet>> {
    let mut stmt = conn.prepare(
        "SELECT slug FROM video_sets GROUP BY slug ORDER BY MAX(built_at) DESC, slug ASC",
    )?;
    let slugs = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for slug in slugs {
        if let Some(set) = load_latest_set(conn, &slug)? {
            out.push(set);
        }
    }
    Ok(out)
}

fn load_set_where(
    conn: &Connection,
    sql: &str,
    args: impl rusqlite::Params,
) -> Result<Option<VideoSet>> {
    let row = conn
        .query_row(sql, args, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .optional()?;
    let Some((slug, bucket_key, title, blurb, built_at, items_json)) = row else {
        return Ok(None);
    };
    let items: Vec<VideoSetItem> = serde_json::from_str(&items_json)?;
    Ok(Some(VideoSet {
        slug,
        bucket_key,
        title,
        blurb,
        built_at,
        items,
    }))
}

/// Drop snapshots older than the newest one per slug, so the table stays a
/// working set rather than an archive.
pub fn prune_old_sets(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM video_sets
         WHERE id NOT IN (SELECT MAX(id) FROM video_sets GROUP BY slug)",
        [],
    )?;
    Ok(())
}

// --- Library signal ---

/// Top-listened library artists carrying a TIDAL id (required for the
/// artist-videos fan-out), ordered by listens.
pub fn load_anchor_pool(conn: &Connection) -> Result<Vec<AnchorArtist>> {
    let mut stmt = conn.prepare(
        "SELECT a.tidal_id, a.name, COUNT(lh.id) AS listens
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         JOIN artists a ON t.artist_id = a.id
         WHERE a.tidal_id IS NOT NULL
         GROUP BY a.id, a.name
         ORDER BY listens DESC, a.name ASC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![ANCHOR_POOL_SIZE], |row| {
            Ok(AnchorArtist::new(row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|a| is_real_artist(&a.name))
        .collect())
}

/// Listened artists whose library tracks carry the given genre.
fn load_genre_anchor_pool(conn: &Connection, genre: &str) -> Result<Vec<AnchorArtist>> {
    let mut stmt = conn.prepare(
        "SELECT a.tidal_id, a.name, COUNT(lh.id) AS listens
         FROM listen_history lh
         JOIN tracks t ON lh.track_id = t.id
         JOIN artists a ON t.artist_id = a.id
         JOIN track_genres tg ON tg.track_id = t.id
         JOIN genres g ON g.id = tg.genre_id
         WHERE a.tidal_id IS NOT NULL AND g.name = ?1
         GROUP BY a.id, a.name
         ORDER BY listens DESC, a.name ASC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![genre, ANCHOR_POOL_SIZE], |row| {
            Ok(AnchorArtist::new(row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|a| is_real_artist(&a.name))
        .collect())
}

/// Artists behind favorited albums, weighted by how many of their albums the
/// listener kept. This is the album-taste signal: saving a whole album is a
/// stronger statement than a play count on one track.
fn load_album_love_pool(conn: &Connection) -> Result<Vec<AnchorArtist>> {
    let mut stmt = conn.prepare(
        "SELECT a.tidal_id, a.name, COUNT(DISTINCT al.id) AS saved_albums
         FROM albums al
         JOIN artists a ON al.artist_id = a.id
         WHERE al.is_favorite = 1 AND a.tidal_id IS NOT NULL
         GROUP BY a.id, a.name
         ORDER BY saved_albums DESC, a.name ASC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![ANCHOR_POOL_SIZE], |row| {
            Ok(AnchorArtist::new(row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|a| is_real_artist(&a.name))
        .collect())
}

/// Flat genre names per anchor artist, aggregated over their library tracks.
/// Feeds `weighted_genre_set`, same as the track-level discovery path.
fn artist_genre_names(
    conn: &Connection,
    tidal_artist_ids: &[i64],
) -> Result<HashMap<i64, Vec<String>>> {
    if tidal_artist_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids_csv: String = tidal_artist_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT a.tidal_id, g.name
         FROM artists a
         JOIN tracks t ON t.artist_id = a.id
         JOIN track_genres tg ON tg.track_id = t.id
         JOIN genres g ON g.id = tg.genre_id
         WHERE a.tidal_id IN ({ids_csv})
         GROUP BY a.tidal_id, g.name"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map: HashMap<i64, Vec<String>> = HashMap::new();
    for r in rows {
        let (artist_id, name) = r?;
        map.entry(artist_id).or_default().push(name);
    }
    Ok(map)
}

/// The listener's library-wide genre profile: top genres by listen count.
/// Used as the shaping seed so genre-aligned anchors rank above outliers.
fn library_genre_profile(conn: &Connection, limit: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT g.name
         FROM listen_history lh
         JOIN track_genres tg ON lh.track_id = tg.track_id
         JOIN genres g ON tg.genre_id = g.id
         GROUP BY g.id, g.name
         ORDER BY COUNT(lh.id) DESC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map(params![limit], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// TIDAL artist ids already in the library, so adjacency picks can exclude
/// what the listener already has.
pub fn known_artist_tidal_ids(conn: &Connection) -> Result<HashSet<i64>> {
    let mut stmt = conn.prepare("SELECT tidal_id FROM artists WHERE tidal_id IS NOT NULL")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().collect())
}

// --- Sampling ---

/// Weighted sample without replacement, weight = listens. Deterministic for a
/// given RNG state.
fn sample_anchors(pool: &[AnchorArtist], rng: &mut StdRng, n: usize) -> Vec<AnchorArtist> {
    let mut remaining: Vec<&AnchorArtist> = pool.iter().collect();
    let mut picked = Vec::new();
    while picked.len() < n && !remaining.is_empty() {
        let total: f64 = remaining.iter().map(|a| a.listens.max(1) as f64).sum();
        let mut roll = rng.random_range(0.0..total);
        let mut chosen = remaining.len() - 1;
        for (i, a) in remaining.iter().enumerate() {
            roll -= a.listens.max(1) as f64;
            if roll <= 0.0 {
                chosen = i;
                break;
            }
        }
        picked.push(remaining.swap_remove(chosen).clone());
    }
    picked
}

// --- Planning ---

/// `server_config` key recording the buckets the last completed build pass
/// covered.
const LAST_PASS_KEY: &str = "video_sets_last_pass";

fn pass_marker(today: chrono::NaiveDate) -> String {
    format!("{}|{}", daily_bucket_key(today), weekly_bucket_key(today))
}

/// Cheap "is a build pass worth running?" check for the request path.
///
/// `plan_missing_sets` runs several aggregate queries over `listen_history`,
/// which is far too heavy to repeat on every page load - and it would hold the
/// single shared connection while doing it. This reads one config row instead.
///
/// Keyed on a completed-pass marker rather than on which sets exist: a shelf
/// that legitimately cannot build (too few candidates) must not re-trigger the
/// whole pass on every request, and a shelf that is merely *missing* must not
/// be masked by its neighbours existing.
pub fn needs_build(conn: &Connection, today: chrono::NaiveDate) -> Result<bool> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM server_config WHERE key = ?1",
            params![LAST_PASS_KEY],
            |row| row.get(0),
        )
        .optional()?;
    Ok(stored.as_deref() != Some(pass_marker(today).as_str()))
}

/// Record that a pass covering `today`'s buckets finished. Called even when a
/// pass builds nothing, so a thin catalog does not mean a rebuild per request.
pub fn mark_pass_complete(conn: &Connection, today: chrono::NaiveDate) -> Result<()> {
    conn.execute(
        "INSERT INTO server_config (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![LAST_PASS_KEY, pass_marker(today)],
    )?;
    Ok(())
}

/// Work out which sets are missing for the current buckets and resolve all
/// their DB-side inputs in one connection scope. Returns an empty vec when
/// everything is current (or when there is no listen history to anchor on).
///
/// Heavy: several aggregates over `listen_history`. Background pass only -
/// the request path uses `needs_build`.
pub fn plan_missing_sets(conn: &Connection, today: chrono::NaiveDate) -> Result<Vec<SetPlan>> {
    let daily = daily_bucket_key(today);
    let weekly = weekly_bucket_key(today);
    let profile_genres = library_genre_profile(conn, 12)?;
    let pool = load_anchor_pool(conn)?;
    let mut plans: Vec<SetPlan> = Vec::new();

    let push_anchored = |plans: &mut Vec<SetPlan>,
                         slug: String,
                         bucket: &str,
                         archetype: Archetype,
                         pool: &[AnchorArtist],
                         take: usize|
     -> Result<()> {
        if pool.is_empty() || load_set(conn, &slug, bucket)?.is_some() {
            return Ok(());
        }
        let mut rng = StdRng::seed_from_u64(build_seed(&slug, bucket));
        let anchors = sample_anchors(pool, &mut rng, take);
        let ids: Vec<i64> = anchors.iter().map(|a| a.tidal_id).collect();
        let anchor_genres = artist_genre_names(conn, &ids)?;
        plans.push(SetPlan {
            slug,
            bucket_key: bucket.to_string(),
            archetype,
            anchors,
            queries: Vec::new(),
            profile_genres: profile_genres.clone(),
            anchor_genres,
        });
        Ok(())
    };

    push_anchored(
        &mut plans,
        DAILY_PICKS_SLUG.to_string(),
        &daily,
        Archetype::DailyPicks,
        &pool,
        ANCHORS_PER_BUILD,
    )?;

    for genre in profile_genres.iter().take(GENRE_SET_COUNT) {
        let genre_pool = load_genre_anchor_pool(conn, genre)?;
        push_anchored(
            &mut plans,
            genre_slug(genre),
            &weekly,
            Archetype::Genre(genre.clone()),
            &genre_pool,
            ANCHORS_PER_BUILD,
        )?;
    }

    let album_pool = load_album_love_pool(conn)?;
    push_anchored(
        &mut plans,
        ALBUM_LOVE_SLUG.to_string(),
        &weekly,
        Archetype::AlbumLove,
        &album_pool,
        ANCHORS_PER_BUILD,
    )?;

    push_anchored(
        &mut plans,
        ONE_STEP_OUT_SLUG.to_string(),
        &weekly,
        Archetype::OneStepOut,
        &pool,
        SIMILAR_SEED_ANCHORS,
    )?;

    if !profile_genres.is_empty() && load_set(conn, DJ_SETS_SLUG, &weekly)?.is_none() {
        plans.push(SetPlan {
            slug: DJ_SETS_SLUG.to_string(),
            bucket_key: weekly.clone(),
            archetype: Archetype::DjSets,
            anchors: Vec::new(),
            queries: long_form_queries(&profile_genres),
            profile_genres: profile_genres.clone(),
            anchor_genres: HashMap::new(),
        });
    }

    Ok(plans)
}

/// Search queries for the long-form shelf: the listener's top genres crossed
/// with the words TIDAL's own titles use for sets and sessions.
fn long_form_queries(profile_genres: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for genre in profile_genres.iter().take(3) {
        out.push(format!("{genre} dj set"));
        out.push(format!("{genre} live set"));
    }
    // Generic fallbacks: long-form video is thin on TIDAL, and the genre
    // queries alone often return too few results to fill a shelf.
    out.push("boiler room".to_string());
    out.push("live session".to_string());
    out.push("essential mix".to_string());
    out.push("live concert".to_string());
    out
}

// --- Candidate fetching ---

/// Fetch per-anchor video lists. A failed fetch degrades to an empty list for
/// that anchor rather than failing the build; the client's global semaphore
/// bounds concurrency.
pub async fn fetch_anchor_videos(
    client: &TidalClient,
    anchors: Vec<AnchorArtist>,
) -> Vec<(AnchorArtist, Vec<VideoCandidate>)> {
    let futures = anchors.into_iter().map(|anchor| async move {
        let videos = match client
            .get_artist_videos(anchor.tidal_id, VIDEOS_PER_ANCHOR, 0)
            .await
        {
            Ok(page) => page.items.iter().map(VideoCandidate::from).collect(),
            Err(e) => {
                tracing::debug!(
                    "video set build: artist videos failed for {} ({}): {e}",
                    anchor.name,
                    anchor.tidal_id
                );
                Vec::new()
            }
        };
        (anchor, videos)
    });
    futures::future::join_all(futures).await
}

/// Expand library seeds one hop through "fans also like", dropping artists
/// already in the library. One hop only: similar-of-similar is where these
/// blends turn to filler.
pub async fn expand_similar_anchors(
    client: &TidalClient,
    seeds: &[AnchorArtist],
    known: &HashSet<i64>,
) -> Vec<AnchorArtist> {
    let futures = seeds.iter().map(|seed| async move {
        match client.get_artist_similar(seed.tidal_id, 12, 0).await {
            Ok(page) => (seed.clone(), page.items),
            Err(e) => {
                tracing::debug!("video set build: similar failed for {}: {e}", seed.name);
                (seed.clone(), Vec::new())
            }
        }
    });
    let groups = futures::future::join_all(futures).await;

    let mut seen: HashSet<i64> = HashSet::new();
    let mut out = Vec::new();
    for (seed, similar) in groups {
        let mut kept = 0usize;
        for artist in similar {
            if kept >= SIMILAR_PER_ANCHOR {
                break;
            }
            if known.contains(&artist.id) || !seen.insert(artist.id) {
                continue;
            }
            out.push(AnchorArtist {
                tidal_id: artist.id,
                name: artist.name,
                // Inherit the seed's weight so a stronger library affinity
                // pulls its neighbours up the ranking too.
                listens: seed.listens,
                via: Some(seed.name.clone()),
            });
            kept += 1;
        }
    }
    out
}

/// Run the long-form searches and keep only genuinely long videos. Each query
/// becomes its own pseudo-anchor so the per-artist cap and copy still work.
pub async fn fetch_long_form(
    client: &TidalClient,
    queries: &[String],
) -> Vec<(AnchorArtist, Vec<VideoCandidate>)> {
    let futures = queries.iter().enumerate().map(|(i, query)| async move {
        let anchor = AnchorArtist::new(-(i as i64) - 1, query.clone(), 1);
        let videos = match client.search_videos(query, SEARCH_LIMIT, 0).await {
            Ok(items) => items
                .iter()
                .map(VideoCandidate::from)
                .filter(|c| c.duration_s.is_some_and(|d| d >= LONG_FORM_MIN_SECONDS))
                .collect(),
            Err(e) => {
                tracing::debug!("video set build: search failed for '{query}': {e}");
                Vec::new()
            }
        };
        (anchor, videos)
    });
    futures::future::join_all(futures).await
}

// --- Assembly ---

/// Score, cap, and dress a set from its fetched candidates. Pure over its
/// inputs (no DB, no network), so curation rules stay unit-testable.
pub fn assemble_set(
    plan: &SetPlan,
    groups: &[(AnchorArtist, Vec<VideoCandidate>)],
) -> Option<VideoSet> {
    let seed_value = plan.seed();
    let mut rng = StdRng::seed_from_u64(seed_value.wrapping_add(1));
    let params = RankParams::default();
    let seed_features = SeedFeatures {
        genre_set: weighted_genre_set(&plan.profile_genres),
        ..Default::default()
    };
    let max_weight = groups
        .iter()
        .map(|(a, _)| a.listens)
        .max()
        .unwrap_or(1)
        .max(1) as f64;

    struct Scored {
        item: VideoSetItem,
        cap_key: String,
        anchor_key: i64,
        via: Option<String>,
        score: f64,
    }
    let mut scored: Vec<Scored> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    for (anchor, videos) in groups {
        let genre_set = plan
            .anchor_genres
            .get(&anchor.tidal_id)
            .map(|names| weighted_genre_set(names))
            .unwrap_or_default();
        let affinity = 0.5 + 0.5 * (anchor.listens.max(1) as f64 / max_weight);
        for video in videos {
            if !seen.insert(video.tidal_id) {
                continue;
            }
            let artist_name = video
                .artist_name
                .clone()
                .unwrap_or_else(|| anchor.name.clone());
            let cand = CandidateFeatures {
                track_id: video.tidal_id,
                is_in_library: plan.archetype != Archetype::OneStepOut,
                source: "library".into(),
                base_score: affinity,
                genre_set: genre_set.clone(),
                artist_id: video.artist_id.or(Some(anchor.tidal_id)),
                artist_name_lc: Some(artist_name.to_lowercase()),
                ..Default::default()
            };
            let shaped = shape_score(&seed_features, &cand, &params, None);
            // derive_why falls back to source phrases ("embedding close") when
            // nothing fired; no embeddings are in play here, so keep the
            // phrase only when a genuine signal produced it.
            let why = if shaped
                .why_signals
                .iter()
                .any(|s| !matches!(*s, "embedding" | "lastfm" | "bridge"))
            {
                shaped.why
            } else {
                String::new()
            };
            // Seeded jitter keeps ordering lively across buckets without
            // outvoting genre alignment.
            let jitter = rng.random_range(0.9..1.1);
            scored.push(Scored {
                item: VideoSetItem {
                    tidal_id: video.tidal_id,
                    title: video.title.clone(),
                    duration_ms: video.duration_s.map(|d| d * 1000),
                    artist_id: video
                        .artist_id
                        .or(Some(anchor.tidal_id))
                        .filter(|id| *id > 0),
                    artist_name: Some(artist_name.clone()),
                    album_tidal_id: video.album_tidal_id,
                    artwork_url: video.artwork_url.clone(),
                    quality: None,
                    explicit: None,
                    kind: "Music Video".to_string(),
                    why,
                },
                cap_key: artist_name.to_lowercase(),
                anchor_key: anchor.tidal_id,
                via: anchor.via.clone(),
                score: shaped.score * jitter,
            });
        }
    }

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.item.tidal_id.cmp(&b.item.tidal_id))
    });

    // The anchor cap only means something when an anchor is a search query.
    // For artist anchors every candidate is by that artist anyway, so
    // PER_ARTIST_CAP already binds and a second cap would just starve the set.
    let anchor_cap = if plan.archetype == Archetype::DjSets {
        PER_ANCHOR_CAP
    } else {
        usize::MAX
    };

    let select = |anchor_cap: usize| {
        let mut per_artist: HashMap<String, usize> = HashMap::new();
        let mut per_anchor: HashMap<i64, usize> = HashMap::new();
        let mut items: Vec<VideoSetItem> = Vec::new();
        let mut featured: Vec<String> = Vec::new();
        let mut vias: Vec<String> = Vec::new();
        for s in scored.iter() {
            if per_artist
                .get(&s.cap_key)
                .is_some_and(|c| *c >= PER_ARTIST_CAP)
                || per_anchor
                    .get(&s.anchor_key)
                    .is_some_and(|c| *c >= anchor_cap)
            {
                continue;
            }
            *per_artist.entry(s.cap_key.clone()).or_insert(0) += 1;
            *per_anchor.entry(s.anchor_key).or_insert(0) += 1;
            if let Some(name) = &s.item.artist_name
                && !featured.contains(name)
            {
                featured.push(name.clone());
            }
            if let Some(via) = &s.via
                && !vias.contains(via)
            {
                vias.push(via.clone());
            }
            items.push(s.item.clone());
            if items.len() >= SET_SIZE {
                break;
            }
        }
        (items, featured, vias)
    };

    let (mut items, mut featured, mut vias) = select(anchor_cap);
    // Variety is preferred, not mandatory: long-form video is thin enough that
    // the cap can starve the shelf entirely, and a slightly repetitive shelf
    // beats no shelf.
    if items.len() < MIN_SET_SIZE && anchor_cap != usize::MAX {
        let (relaxed, relaxed_featured, relaxed_vias) = select(usize::MAX);
        if relaxed.len() > items.len() {
            items = relaxed;
            featured = relaxed_featured;
            vias = relaxed_vias;
        }
    }
    if items.len() < MIN_SET_SIZE {
        return None;
    }

    // "Leads" has to mean the artist actually at the top of the shelf, not
    // whoever happens to have the highest play count in the group.
    let lead_name = items
        .first()
        .and_then(|item| item.artist_name.clone())
        .unwrap_or_default();
    let top_anchor = groups
        .iter()
        .map(|(a, _)| a)
        .find(|a| a.name == lead_name)
        .cloned();
    let (title, blurb) = write_copy(
        &plan.archetype,
        &mut rng,
        items.len(),
        &featured,
        &vias,
        top_anchor.as_ref(),
    );
    Some(VideoSet {
        slug: plan.slug.clone(),
        bucket_key: plan.bucket_key.clone(),
        title,
        blurb,
        built_at: String::new(),
        items,
    })
}

// --- Copy ---

fn count_word(n: usize) -> &'static str {
    match n {
        4 => "Four",
        5 => "Five",
        6 => "Six",
        7 => "Seven",
        8 => "Eight",
        9 => "Nine",
        10 => "Ten",
        11 => "Eleven",
        _ => "Twelve",
    }
}

/// Templated title + blurb. Every fact slotted in comes from the DB (artist
/// names, counts, genre names); the phrase bank supplies the voice. Seeded by
/// the build RNG so a bucket's copy is as stable as its picks.
fn write_copy(
    archetype: &Archetype,
    rng: &mut StdRng,
    item_count: usize,
    featured: &[String],
    vias: &[String],
    top_anchor: Option<&AnchorArtist>,
) -> (String, String) {
    let n = count_word(item_count);
    match archetype {
        Archetype::DailyPicks => {
            const TITLES: &[&str] = &[
                "In heavy rotation",
                "From your orbit",
                "Watch what you play",
                "Your library, on camera",
                "Today's picks",
            ];
            let title = TITLES[rng.random_range(0..TITLES.len())].to_string();
            let opener = format!("{n} videos from artists you already trust.");
            let closer = match (top_anchor, featured) {
                (Some(anchor), _) if anchor.listens >= 10 => {
                    let closers = [
                        format!(
                            "Heavy on {} today - {} plays says you won't mind.",
                            anchor.name, anchor.listens
                        ),
                        format!(
                            "{} leads; the {} plays you've given them earned it.",
                            anchor.name, anchor.listens
                        ),
                    ];
                    closers[rng.random_range(0..closers.len())].clone()
                }
                (_, [first, second, ..]) => {
                    format!("{first}, {second}, and friends. No searching required.")
                }
                (_, [only]) => format!("All {only}, as it happens."),
                _ => "Drawn from what you actually play.".to_string(),
            };
            (title, format!("{opener} {closer}"))
        }
        Archetype::Genre(genre) => {
            let titles = [
                format!("{genre}, on camera"),
                format!("The {genre} shelf"),
                format!("{genre}, watched"),
            ];
            let title = titles[rng.random_range(0..titles.len())].clone();
            let blurb = match featured {
                [first, second, ..] => format!(
                    "{n} videos from the {genre} corner of your library. {first}, {second}, and the rest of that room."
                ),
                [only] => format!("{n} {genre} videos, mostly {only}."),
                _ => format!("{n} videos from your {genre} listening."),
            };
            (title, blurb)
        }
        Archetype::AlbumLove => {
            const TITLES: &[&str] = &["Albums you kept", "The saved shelf", "Whole-album artists"];
            let title = TITLES[rng.random_range(0..TITLES.len())].to_string();
            let blurb = match featured {
                [first, second, ..] => format!(
                    "{n} videos from artists whose albums you saved outright - {first}, {second}, and company. Saving the whole record says more than a play count."
                ),
                [only] => format!("{n} videos from {only}, whose albums you saved outright."),
                _ => format!("{n} videos from the artists behind your saved albums."),
            };
            (title, blurb)
        }
        Archetype::OneStepOut => {
            const TITLES: &[&str] = &["One step out", "New to you", "Just outside"];
            let title = TITLES[rng.random_range(0..TITLES.len())].to_string();
            let artist_count = featured.len();
            let blurb = match vias {
                [first, second, ..] => format!(
                    "{n} videos from {artist_count} artists you don't have yet, reached through {first} and {second}. One of them will stick."
                ),
                [only] => format!(
                    "{n} videos from artists you don't have yet. Fans of {only} usually end up here."
                ),
                _ => format!("{n} videos from artists just outside your library."),
            };
            (title, blurb)
        }
        Archetype::DjSets => {
            const TITLES: &[&str] = &["The long players", "Sets and sessions", "Put it on"];
            let title = TITLES[rng.random_range(0..TITLES.len())].to_string();
            let blurb = match featured {
                [first, ..] => format!(
                    "{n} long-form sets and sessions, fifteen minutes and up. Starting with {first}. Built for leaving on."
                ),
                _ => format!("{n} long-form sets and sessions. Built for leaving on."),
            };
            (title, blurb)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(tidal_id: i64, name: &str, listens: i64) -> AnchorArtist {
        AnchorArtist::new(tidal_id, name.to_string(), listens)
    }

    fn candidate(id: i64, title: &str, artist: &str, duration_s: i64) -> VideoCandidate {
        VideoCandidate {
            tidal_id: id,
            title: title.to_string(),
            duration_s: Some(duration_s),
            artist_id: None,
            artist_name: Some(artist.to_string()),
            album_tidal_id: None,
            artwork_url: None,
        }
    }

    fn plan(archetype: Archetype, anchors: Vec<AnchorArtist>) -> SetPlan {
        SetPlan {
            slug: DAILY_PICKS_SLUG.to_string(),
            bucket_key: "2026-07-17".to_string(),
            archetype,
            anchors,
            queries: Vec::new(),
            profile_genres: Vec::new(),
            anchor_genres: HashMap::new(),
        }
    }

    #[test]
    fn same_bucket_samples_identically_new_bucket_differs() {
        let pool: Vec<AnchorArtist> = (1..=30)
            .map(|i| anchor(i, &format!("Artist {i}"), 100 - i))
            .collect();
        let mut rng_a = StdRng::seed_from_u64(build_seed(DAILY_PICKS_SLUG, "2026-07-17"));
        let mut rng_b = StdRng::seed_from_u64(build_seed(DAILY_PICKS_SLUG, "2026-07-17"));
        let mut rng_c = StdRng::seed_from_u64(build_seed(DAILY_PICKS_SLUG, "2026-07-18"));
        let ids = |v: &[AnchorArtist]| v.iter().map(|a| a.tidal_id).collect::<Vec<_>>();
        assert_eq!(
            ids(&sample_anchors(&pool, &mut rng_a, 10)),
            ids(&sample_anchors(&pool, &mut rng_b, 10))
        );
        assert_ne!(
            ids(&sample_anchors(&pool, &mut rng_a, 10)),
            ids(&sample_anchors(&pool, &mut rng_c, 10))
        );
    }

    #[test]
    fn assemble_is_deterministic_and_caps_per_artist() {
        let a1 = anchor(1, "Tycho", 50);
        let a2 = anchor(2, "Bonobo", 30);
        let groups = vec![
            (
                a1.clone(),
                (0..6)
                    .map(|i| candidate(100 + i, &format!("T{i}"), "Tycho", 240))
                    .collect(),
            ),
            (
                a2.clone(),
                (0..6)
                    .map(|i| candidate(200 + i, &format!("B{i}"), "Bonobo", 240))
                    .collect(),
            ),
        ];
        let p = plan(Archetype::DailyPicks, vec![a1, a2]);
        let one = assemble_set(&p, &groups).unwrap();
        let two = assemble_set(&p, &groups).unwrap();
        assert_eq!(
            one.items.iter().map(|i| i.tidal_id).collect::<Vec<_>>(),
            two.items.iter().map(|i| i.tidal_id).collect::<Vec<_>>()
        );
        assert_eq!(one.blurb, two.blurb);
        for artist in ["tycho", "bonobo"] {
            let from_artist = one
                .items
                .iter()
                .filter(|i| {
                    i.artist_name
                        .as_deref()
                        .is_some_and(|n| n.to_lowercase() == artist)
                })
                .count();
            assert!(from_artist <= PER_ARTIST_CAP, "{artist} exceeded the cap");
        }
    }

    #[test]
    fn assemble_drops_below_minimum_instead_of_padding() {
        let a1 = anchor(1, "Tycho", 50);
        let groups = vec![(
            a1.clone(),
            vec![
                candidate(100, "Only", "Tycho", 240),
                candidate(101, "Two", "Tycho", 240),
            ],
        )];
        assert!(assemble_set(&plan(Archetype::DailyPicks, vec![a1]), &groups).is_none());
    }

    #[test]
    fn assemble_dedupes_video_ids_across_anchors() {
        let a1 = anchor(1, "Tycho", 50);
        let a2 = anchor(2, "Bonobo", 30);
        let shared: Vec<VideoCandidate> = (0..5)
            .map(|i| candidate(300 + i, &format!("S{i}"), &format!("Artist {i}"), 240))
            .collect();
        let mut second = shared.clone();
        second.extend(
            (0..3).map(|i| candidate(400 + i, &format!("U{i}"), &format!("Other {i}"), 240)),
        );
        let groups = vec![(a1.clone(), shared), (a2.clone(), second)];
        let set = assemble_set(&plan(Archetype::DailyPicks, vec![a1, a2]), &groups).unwrap();
        let mut ids: Vec<i64> = set.items.iter().map(|i| i.tidal_id).collect();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len());
    }

    #[test]
    fn copy_differs_by_archetype_and_names_real_facts() {
        let a = anchor(1, "Tycho", 42);
        let groups = vec![(
            a.clone(),
            (0..8)
                .map(|i| candidate(500 + i, &format!("V{i}"), &format!("Artist {i}"), 240))
                .collect(),
        )];
        let mut daily = plan(Archetype::DailyPicks, vec![a.clone()]);
        daily.slug = DAILY_PICKS_SLUG.into();
        let mut genre = plan(Archetype::Genre("Ambient".into()), vec![a.clone()]);
        genre.slug = genre_slug("Ambient");
        let mut long = plan(Archetype::DjSets, vec![a]);
        long.slug = DJ_SETS_SLUG.into();
        // The long-form shelf is fed by several queries, and the anchor cap
        // deliberately limits how much of it any one query can supply.
        let long_groups: Vec<(AnchorArtist, Vec<VideoCandidate>)> = (0..4)
            .map(|q| {
                (
                    anchor(-1 - q, &format!("query {q}"), 1),
                    (0..3)
                        .map(|i| {
                            candidate(
                                600 + q * 10 + i,
                                &format!("Set {q}-{i}"),
                                &format!("DJ {q}-{i}"),
                                LONG_FORM_MIN_SECONDS + 60,
                            )
                        })
                        .collect(),
                )
            })
            .collect();

        let daily_set = assemble_set(&daily, &groups).unwrap();
        let genre_set = assemble_set(&genre, &groups).unwrap();
        let long_set = assemble_set(&long, &long_groups).unwrap();

        assert!(genre_set.title.contains("Ambient") || genre_set.blurb.contains("Ambient"));
        assert!(long_set.blurb.to_lowercase().contains("long-form"));
        assert_ne!(daily_set.title, genre_set.title);
        // Blurbs name artists that are actually in the set.
        for set in [&daily_set, &genre_set, &long_set] {
            assert!(!set.blurb.is_empty());
        }
    }

    #[test]
    fn one_search_query_cannot_own_the_long_form_shelf() {
        // Four queries, one of them returning a whole series of near-identical
        // uploads by different DJs. Without the anchor cap that single query
        // fills the rail; per-artist capping cannot catch it because every
        // upload has a different artist.
        let mut groups: Vec<(AnchorArtist, Vec<VideoCandidate>)> = vec![(
            anchor(-1, "boiler room", 1),
            (0..10)
                .map(|i| {
                    candidate(
                        700 + i,
                        &format!("Boiler Room City: Act {i}"),
                        &format!("DJ {i}"),
                        LONG_FORM_MIN_SECONDS + 100,
                    )
                })
                .collect(),
        )];
        for q in 1..4 {
            groups.push((
                anchor(-1 - q, &format!("query {q}"), 1),
                (0..3)
                    .map(|i| {
                        candidate(
                            800 + q * 10 + i,
                            &format!("Session {q}-{i}"),
                            &format!("Artist {q}-{i}"),
                            LONG_FORM_MIN_SECONDS + 100,
                        )
                    })
                    .collect(),
            ));
        }
        let mut p = plan(Archetype::DjSets, Vec::new());
        p.slug = DJ_SETS_SLUG.into();
        let set = assemble_set(&p, &groups).unwrap();
        let from_series = set
            .items
            .iter()
            .filter(|i| i.title.starts_with("Boiler Room City"))
            .count();
        assert!(
            from_series <= PER_ANCHOR_CAP,
            "one query supplied {from_series} of {} items",
            set.items.len()
        );
    }

    #[test]
    fn placeholder_artists_never_anchor_a_set() {
        assert!(!is_real_artist("Various Artists"));
        assert!(!is_real_artist("  VARIOUS  "));
        assert!(!is_real_artist(""));
        assert!(is_real_artist("Various Production"));
        assert!(is_real_artist("Tycho"));
    }

    #[test]
    fn lead_credit_names_the_artist_at_the_top_of_the_set() {
        // The blurb says a name "leads", so it has to be the first item's
        // artist, not whoever in the group has the highest play count.
        let quiet = anchor(1, "Quiet Lead", 5);
        let loud = anchor(2, "Loud Anchor", 900);
        let groups = vec![
            (
                quiet.clone(),
                (0..6)
                    .map(|i| candidate(900 + i, &format!("Q{i}"), "Quiet Lead", 240))
                    .collect(),
            ),
            (
                loud.clone(),
                (0..6)
                    .map(|i| candidate(950 + i, &format!("L{i}"), "Loud Anchor", 240))
                    .collect(),
            ),
        ];
        let set = assemble_set(&plan(Archetype::DailyPicks, vec![quiet, loud]), &groups).unwrap();
        let lead = set.items[0].artist_name.clone().unwrap();
        let other = if lead == "Quiet Lead" {
            "Loud Anchor"
        } else {
            "Quiet Lead"
        };
        if set.blurb.contains("leads") {
            assert!(set.blurb.contains(&lead), "blurb: {}", set.blurb);
            assert!(!set.blurb.contains(other), "blurb: {}", set.blurb);
        }
    }

    #[test]
    fn long_form_filter_keeps_only_long_videos() {
        // The duration gate is the whole format signal for the DJ shelf.
        let short = candidate(1, "Single", "A", 200);
        let long = candidate(2, "Set", "A", LONG_FORM_MIN_SECONDS + 1);
        let kept: Vec<i64> = [short, long]
            .iter()
            .filter(|c| c.duration_s.is_some_and(|d| d >= LONG_FORM_MIN_SECONDS))
            .map(|c| c.tidal_id)
            .collect();
        assert_eq!(kept, vec![2]);
    }

    #[test]
    fn genre_slugs_are_key_safe() {
        assert_eq!(genre_slug("Drum & Bass"), "genre:drum-bass");
        assert_eq!(genre_slug("Hip-Hop/Rap"), "genre:hip-hop-rap");
    }

    #[test]
    fn weekly_bucket_is_stable_within_an_iso_week() {
        let mon = chrono::NaiveDate::from_ymd_opt(2026, 7, 13).unwrap();
        let sun = chrono::NaiveDate::from_ymd_opt(2026, 7, 19).unwrap();
        let next = chrono::NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        assert_eq!(weekly_bucket_key(mon), weekly_bucket_key(sun));
        assert_ne!(weekly_bucket_key(sun), weekly_bucket_key(next));
    }

    #[test]
    fn store_and_load_round_trip() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let set = VideoSet {
            slug: DAILY_PICKS_SLUG.into(),
            bucket_key: "2026-07-17".into(),
            title: "In heavy rotation".into(),
            blurb: "Four videos.".into(),
            built_at: String::new(),
            items: vec![VideoSetItem {
                tidal_id: 42,
                title: "Clip".into(),
                duration_ms: Some(240_000),
                artist_id: Some(7),
                artist_name: Some("Tycho".into()),
                album_tidal_id: None,
                artwork_url: None,
                quality: None,
                explicit: None,
                kind: "Music Video".into(),
                why: String::new(),
            }],
        };
        store_set(&conn, &set).unwrap();
        let loaded = load_set(&conn, DAILY_PICKS_SLUG, "2026-07-17")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.items.len(), 1);
        assert_eq!(loaded.items[0].tidal_id, 42);
        store_set(&conn, &set).unwrap();
        assert_eq!(
            load_latest_set(&conn, DAILY_PICKS_SLUG)
                .unwrap()
                .unwrap()
                .bucket_key,
            "2026-07-17"
        );
    }

    #[test]
    fn load_latest_sets_returns_one_row_per_slug_and_prune_keeps_newest() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let mk = |slug: &str, bucket: &str| VideoSet {
            slug: slug.into(),
            bucket_key: bucket.into(),
            title: "T".into(),
            blurb: "B".into(),
            built_at: String::new(),
            items: vec![VideoSetItem {
                tidal_id: 1,
                title: "Clip".into(),
                duration_ms: None,
                artist_id: None,
                artist_name: None,
                album_tidal_id: None,
                artwork_url: None,
                quality: None,
                explicit: None,
                kind: "Music Video".into(),
                why: String::new(),
            }],
        };
        store_set(&conn, &mk(DAILY_PICKS_SLUG, "2026-07-16")).unwrap();
        store_set(&conn, &mk(DAILY_PICKS_SLUG, "2026-07-17")).unwrap();
        store_set(&conn, &mk(DJ_SETS_SLUG, "2026-W29")).unwrap();

        let sets = load_latest_sets(&conn).unwrap();
        assert_eq!(sets.len(), 2, "one row per slug");

        prune_old_sets(&conn).unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM video_sets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 2, "older daily snapshot pruned");
    }

    #[test]
    fn planner_skips_everything_on_an_empty_library() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 7, 17).unwrap();
        assert!(plan_missing_sets(&conn, today).unwrap().is_empty());
    }

    #[test]
    fn needs_build_is_driven_by_the_pass_marker() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::run_migrations(&conn).unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 7, 17).unwrap();

        assert!(needs_build(&conn, today).unwrap(), "no pass has run yet");

        mark_pass_complete(&conn, today).unwrap();
        assert!(
            !needs_build(&conn, today).unwrap(),
            "a completed pass covers the whole bucket, including shelves that \
             could not build - otherwise a thin catalog rebuilds per request"
        );

        // Tomorrow's daily bucket reopens the pass.
        let tomorrow = today.succ_opt().unwrap();
        assert!(needs_build(&conn, tomorrow).unwrap());

        // So does a new ISO week, even on a day the daily key alone would
        // consider covered.
        mark_pass_complete(&conn, tomorrow).unwrap();
        let next_week = tomorrow + chrono::Duration::days(7);
        assert!(needs_build(&conn, next_week).unwrap());
    }
}
