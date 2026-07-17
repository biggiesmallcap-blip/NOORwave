//! Editorial video set builder for the /videos browse state.
//!
//! A "set" is a small curated snapshot (5-12 videos) built for one rotation
//! bucket (a calendar day for daily sets) and persisted whole in `video_sets`.
//! Rebuilding the same (slug, bucket_key) against the same library yields the
//! same set: anchor sampling, scoring jitter, and copy all draw from one RNG
//! seeded on the slug + bucket key, so the page is stable across reloads
//! within a bucket and fresh across buckets.
//!
//! Candidate generation fans out over `get_artist_videos` for library anchor
//! artists (the TIDAL client's own 4-inflight semaphore bounds the burst) and
//! scores with the shared `discovery_ranking::shape_score`: the seed is the
//! listener's library-wide genre profile, the candidate carries the anchor
//! artist's genre set, and every DSP multiplier passes through neutral, per
//! that module's missing-data contract.

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
use crate::services::tidal::client::{TidalArtistVideo, TidalClient};

pub const DAILY_PICKS_SLUG: &str = "daily-picks";

/// How many top-listened artists form the sampling pool. Wide enough that the
/// daily draw feels different day to day, narrow enough to stay "artists you
/// already trust".
const ANCHOR_POOL_SIZE: i64 = 60;
/// Anchors drawn per daily build. Each costs one TIDAL artist-videos call.
const ANCHORS_PER_BUILD: usize = 10;
/// Videos requested per anchor.
const VIDEOS_PER_ANCHOR: i32 = 10;
/// At most this many picks from one artist, so nobody owns the mural.
const PER_ARTIST_CAP: usize = 2;
/// Target set size; the builder drops rather than pads below this.
const SET_SIZE: usize = 12;
/// A set smaller than this is not worth showing; the route treats it as absent.
const MIN_SET_SIZE: usize = 4;

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

/// Anchor candidate: a library artist with listen history and a TIDAL id.
#[derive(Debug, Clone)]
pub struct AnchorArtist {
    pub tidal_id: i64,
    pub name: String,
    pub listens: i64,
}

/// Bucket key for daily sets: the local calendar date.
pub fn daily_bucket_key(date: chrono::NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
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

// --- Library signal ---

/// Top-listened library artists that carry a TIDAL id (required for the
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
            Ok(AnchorArtist {
                tidal_id: row.get(0)?,
                name: row.get(1)?,
                listens: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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
fn library_genre_profile(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT g.name
         FROM listen_history lh
         JOIN track_genres tg ON lh.track_id = tg.track_id
         JOIN genres g ON tg.genre_id = g.id
         GROUP BY g.id, g.name
         ORDER BY COUNT(lh.id) DESC
         LIMIT 12",
    )?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// --- Sampling and scoring ---

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

/// Inputs snapshot read from the DB in one lock scope, so the async fan-out
/// below never holds the connection.
pub struct DailyBuildInputs {
    pub anchors: Vec<AnchorArtist>,
    pub anchor_genres: HashMap<i64, Vec<String>>,
    pub profile_genres: Vec<String>,
}

/// Read everything the daily build needs from the DB. Returns `None` when the
/// library has no listen history to anchor on (fresh install, no TIDAL sync).
pub fn read_daily_build_inputs(conn: &Connection, seed: u64) -> Result<Option<DailyBuildInputs>> {
    let pool = load_anchor_pool(conn)?;
    if pool.is_empty() {
        return Ok(None);
    }
    let mut rng = StdRng::seed_from_u64(seed);
    let anchors = sample_anchors(&pool, &mut rng, ANCHORS_PER_BUILD);
    let ids: Vec<i64> = anchors.iter().map(|a| a.tidal_id).collect();
    let anchor_genres = artist_genre_names(conn, &ids)?;
    let profile_genres = library_genre_profile(conn)?;
    Ok(Some(DailyBuildInputs {
        anchors,
        anchor_genres,
        profile_genres,
    }))
}

/// Assemble the daily picks set from pre-fetched inputs and per-anchor video
/// lists. Pure over its inputs (no DB, no network), so the determinism and
/// curation rules are unit-testable.
pub fn assemble_daily_picks(
    bucket_key: &str,
    seed: u64,
    inputs: &DailyBuildInputs,
    videos_by_anchor: &[(AnchorArtist, Vec<TidalArtistVideo>)],
) -> Option<VideoSet> {
    let mut rng = StdRng::seed_from_u64(seed.wrapping_add(1));
    let params = RankParams::default();
    let seed_features = SeedFeatures {
        genre_set: weighted_genre_set(&inputs.profile_genres),
        ..Default::default()
    };
    let max_listens = inputs
        .anchors
        .iter()
        .map(|a| a.listens)
        .max()
        .unwrap_or(1)
        .max(1) as f64;

    struct Scored {
        item: VideoSetItem,
        anchor_tidal_id: i64,
        score: f64,
    }
    let mut scored: Vec<Scored> = Vec::new();
    let mut seen: HashSet<i64> = HashSet::new();
    for (anchor, videos) in videos_by_anchor {
        let genre_set = inputs
            .anchor_genres
            .get(&anchor.tidal_id)
            .map(|names| weighted_genre_set(names))
            .unwrap_or_default();
        let affinity = 0.5 + 0.5 * (anchor.listens.max(1) as f64 / max_listens);
        for video in videos {
            if !seen.insert(video.id) {
                continue;
            }
            let cand = CandidateFeatures {
                track_id: video.id,
                is_in_library: true,
                source: "library".into(),
                base_score: affinity,
                genre_set: genre_set.clone(),
                artist_id: Some(anchor.tidal_id),
                artist_name_lc: Some(anchor.name.to_lowercase()),
                ..Default::default()
            };
            let shaped = shape_score(&seed_features, &cand, &params, None);
            // derive_why falls back to source phrases ("embedding close")
            // when nothing fired; no embeddings are in play here, so keep
            // the phrase only when a genuine signal produced it.
            let why = if shaped
                .why_signals
                .iter()
                .any(|s| !matches!(*s, "embedding" | "lastfm" | "bridge"))
            {
                shaped.why
            } else {
                String::new()
            };
            // Seeded jitter keeps intra-artist ordering lively across days
            // without letting it outvote genre alignment.
            let jitter = rng.random_range(0.9..1.1);
            let artist_name = video
                .artist
                .as_ref()
                .map(|a| a.name.clone())
                .unwrap_or_else(|| anchor.name.clone());
            scored.push(Scored {
                item: VideoSetItem {
                    tidal_id: video.id,
                    title: video.title.clone(),
                    duration_ms: Some(video.duration * 1000),
                    artist_id: Some(anchor.tidal_id),
                    artist_name: Some(artist_name),
                    album_tidal_id: video.album.as_ref().map(|al| al.id),
                    artwork_url: TidalClient::get_artwork_url(&video.image_id, 640),
                    quality: None,
                    explicit: None,
                    kind: video
                        .extra
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("Music Video")
                        .to_string(),
                    why,
                },
                anchor_tidal_id: anchor.tidal_id,
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

    let mut per_artist: HashMap<i64, usize> = HashMap::new();
    let mut items: Vec<VideoSetItem> = Vec::new();
    let mut featured: Vec<String> = Vec::new();
    for s in scored {
        let count = per_artist.entry(s.anchor_tidal_id).or_insert(0);
        if *count >= PER_ARTIST_CAP {
            continue;
        }
        *count += 1;
        if let Some(name) = &s.item.artist_name
            && !featured.contains(name)
        {
            featured.push(name.clone());
        }
        items.push(s.item);
        if items.len() >= SET_SIZE {
            break;
        }
    }
    if items.len() < MIN_SET_SIZE {
        return None;
    }

    let top_anchor = inputs
        .anchors
        .iter()
        .filter(|a| featured.contains(&a.name))
        .max_by_key(|a| a.listens);
    let (title, blurb) = daily_copy(&mut rng, items.len(), &featured, top_anchor);
    Some(VideoSet {
        slug: DAILY_PICKS_SLUG.to_string(),
        bucket_key: bucket_key.to_string(),
        title,
        blurb,
        built_at: String::new(),
        items,
    })
}

/// Fetch per-anchor video lists. A failed fetch degrades to an empty list for
/// that anchor rather than failing the build; the client's global semaphore
/// bounds concurrency.
pub async fn fetch_anchor_videos(
    client: &TidalClient,
    anchors: Vec<AnchorArtist>,
) -> Vec<(AnchorArtist, Vec<TidalArtistVideo>)> {
    let futures = anchors.into_iter().map(|anchor| async move {
        let videos = match client
            .get_artist_videos(anchor.tidal_id, VIDEOS_PER_ANCHOR, 0)
            .await
        {
            Ok(page) => page.items,
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

// --- Copy ---

/// Templated title + blurb. Every fact slotted in comes from the DB (artist
/// names, counts); the phrase bank supplies the voice. Seeded by the build
/// RNG so the day's copy is as stable as the day's picks.
fn daily_copy(
    rng: &mut StdRng,
    item_count: usize,
    featured: &[String],
    top_anchor: Option<&AnchorArtist>,
) -> (String, String) {
    const TITLES: &[&str] = &[
        "In heavy rotation",
        "From your orbit",
        "Watch what you play",
        "Your library, on camera",
        "Today's picks",
    ];
    let title = TITLES[rng.random_range(0..TITLES.len())].to_string();

    let count_word = match item_count {
        4 => "Four",
        5 => "Five",
        6 => "Six",
        7 => "Seven",
        8 => "Eight",
        9 => "Nine",
        10 => "Ten",
        11 => "Eleven",
        _ => "Twelve",
    };
    let opener = format!("{count_word} videos from artists you already trust.");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(tidal_id: i64, name: &str, listens: i64) -> AnchorArtist {
        AnchorArtist {
            tidal_id,
            name: name.to_string(),
            listens,
        }
    }

    fn video(id: i64, title: &str) -> TidalArtistVideo {
        TidalArtistVideo {
            id,
            title: title.to_string(),
            duration: 240,
            image_id: None,
            artist: None,
            album: None,
            extra: HashMap::new(),
        }
    }

    fn inputs(anchors: Vec<AnchorArtist>) -> DailyBuildInputs {
        DailyBuildInputs {
            anchors,
            anchor_genres: HashMap::new(),
            profile_genres: Vec::new(),
        }
    }

    #[test]
    fn same_seed_same_set_different_seed_different_order() {
        let pool: Vec<AnchorArtist> = (1..=30)
            .map(|i| anchor(i, &format!("Artist {i}"), 100 - i))
            .collect();
        let mut rng_a = StdRng::seed_from_u64(build_seed(DAILY_PICKS_SLUG, "2026-07-17"));
        let mut rng_b = StdRng::seed_from_u64(build_seed(DAILY_PICKS_SLUG, "2026-07-17"));
        let mut rng_c = StdRng::seed_from_u64(build_seed(DAILY_PICKS_SLUG, "2026-07-18"));
        let a = sample_anchors(&pool, &mut rng_a, 10);
        let b = sample_anchors(&pool, &mut rng_b, 10);
        let c = sample_anchors(&pool, &mut rng_c, 10);
        let ids = |v: &[AnchorArtist]| v.iter().map(|a| a.tidal_id).collect::<Vec<_>>();
        assert_eq!(ids(&a), ids(&b), "same bucket must sample identically");
        assert_ne!(ids(&a), ids(&c), "a new bucket should draw differently");
    }

    #[test]
    fn assemble_is_deterministic_and_caps_per_artist() {
        let a1 = anchor(1, "Tycho", 50);
        let a2 = anchor(2, "Bonobo", 30);
        let videos_by_anchor = vec![
            (
                a1.clone(),
                (0..6).map(|i| video(100 + i, &format!("T{i}"))).collect(),
            ),
            (
                a2.clone(),
                (0..6).map(|i| video(200 + i, &format!("B{i}"))).collect(),
            ),
        ];
        let ins = inputs(vec![a1, a2]);
        let seed = build_seed(DAILY_PICKS_SLUG, "2026-07-17");
        let one = assemble_daily_picks("2026-07-17", seed, &ins, &videos_by_anchor).unwrap();
        let two = assemble_daily_picks("2026-07-17", seed, &ins, &videos_by_anchor).unwrap();
        assert_eq!(
            one.items.iter().map(|i| i.tidal_id).collect::<Vec<_>>(),
            two.items.iter().map(|i| i.tidal_id).collect::<Vec<_>>()
        );
        assert_eq!(one.title, two.title);
        assert_eq!(one.blurb, two.blurb);
        for anchor_id in [1, 2] {
            let from_artist = one
                .items
                .iter()
                .filter(|i| i.artist_id == Some(anchor_id))
                .count();
            assert!(from_artist <= PER_ARTIST_CAP);
        }
    }

    #[test]
    fn assemble_drops_below_minimum_instead_of_padding() {
        let a1 = anchor(1, "Tycho", 50);
        let videos_by_anchor = vec![(a1.clone(), vec![video(100, "Only"), video(101, "Two")])];
        let ins = inputs(vec![a1]);
        let seed = build_seed(DAILY_PICKS_SLUG, "2026-07-17");
        assert!(assemble_daily_picks("2026-07-17", seed, &ins, &videos_by_anchor).is_none());
    }

    #[test]
    fn assemble_dedupes_video_ids_across_anchors() {
        let a1 = anchor(1, "Tycho", 50);
        let a2 = anchor(2, "Bonobo", 30);
        let shared: Vec<TidalArtistVideo> =
            (0..5).map(|i| video(300 + i, &format!("S{i}"))).collect();
        let mut second = shared.clone();
        second.extend((0..3).map(|i| video(400 + i, &format!("U{i}"))));
        let videos_by_anchor = vec![(a1.clone(), shared), (a2.clone(), second)];
        let ins = inputs(vec![a1, a2]);
        let seed = build_seed(DAILY_PICKS_SLUG, "2026-07-17");
        let set = assemble_daily_picks("2026-07-17", seed, &ins, &videos_by_anchor).unwrap();
        let mut ids: Vec<i64> = set.items.iter().map(|i| i.tidal_id).collect();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len());
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
        // Upsert replaces in place.
        store_set(&conn, &set).unwrap();
        let latest = load_latest_set(&conn, DAILY_PICKS_SLUG).unwrap().unwrap();
        assert_eq!(latest.bucket_key, "2026-07-17");
    }
}
