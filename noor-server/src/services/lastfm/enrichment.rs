use anyhow::Result;
use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use crate::SharedState;
use crate::genre::mappings::GenreCatalog;
use crate::genre::scorer::{MIN_SCORE_FLOOR, TagInput, TagLevel, TagSource, score_genre_tags};
use crate::metadata::lastfm::LastFmClient;
use crate::services::lastfm::tag_filter::is_artist_name_tag;
use crate::tags::context::{TagContext, classify_tag_context};
use rusqlite::Connection;

const CALL_DELAY_MS: u64 = 200;
const MAX_RETRIES: u32 = 3;

type CountedTag = (String, Option<u32>);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnrichmentMode {
    Pending,
    RetryUntagged,
    RefreshAll,
}

pub struct EnrichmentStats {
    pub total_tracks: i64,
    pub checked_tracks: i64,
    pub enriched_tracks: i64,
    pub remaining_tracks: i64,
}

pub fn count_tracks_to_enrich(conn: &Connection, mode: EnrichmentMode) -> Result<usize> {
    let sql = match mode {
        EnrichmentMode::Pending => {
            "SELECT COUNT(*) FROM tracks t
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
               AND NOT EXISTS (SELECT 1 FROM lastfm_checked lc WHERE lc.track_id = t.id)"
        }
        EnrichmentMode::RetryUntagged => {
            "SELECT COUNT(*) FROM tracks t
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
               AND EXISTS (SELECT 1 FROM lastfm_checked lc WHERE lc.track_id = t.id)
               AND NOT EXISTS (
                   SELECT 1 FROM track_genres tg
                   WHERE tg.track_id = t.id AND tg.source = 'lastfm'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM track_context_tags tct
                   WHERE tct.track_id = t.id AND tct.source = 'lastfm'
               )"
        }
        EnrichmentMode::RefreshAll => {
            "SELECT COUNT(*) FROM tracks t
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))"
        }
    };
    let count: i64 = conn.query_row(sql, [], |row| row.get(0))?;
    Ok(count as usize)
}

pub fn load_tracks_to_enrich(
    conn: &Connection,
    mode: EnrichmentMode,
) -> Result<Vec<(i64, String, String)>> {
    let sql = match mode {
        EnrichmentMode::Pending => {
            "SELECT t.id, t.title, a.name
             FROM tracks t
             JOIN artists a ON t.artist_id = a.id
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
               AND NOT EXISTS (
                   SELECT 1 FROM lastfm_checked lc WHERE lc.track_id = t.id
               )
             ORDER BY t.id"
        }
        EnrichmentMode::RetryUntagged => {
            "SELECT t.id, t.title, a.name
             FROM tracks t
             JOIN artists a ON t.artist_id = a.id
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
               AND EXISTS (
                   SELECT 1 FROM lastfm_checked lc WHERE lc.track_id = t.id
               )
               AND NOT EXISTS (
                   SELECT 1 FROM track_genres tg
                   WHERE tg.track_id = t.id AND tg.source = 'lastfm'
               )
               AND NOT EXISTS (
                   SELECT 1 FROM track_context_tags tct
                   WHERE tct.track_id = t.id AND tct.source = 'lastfm'
               )
             ORDER BY t.id"
        }
        EnrichmentMode::RefreshAll => {
            "SELECT t.id, t.title, a.name
             FROM tracks t
             JOIN artists a ON t.artist_id = a.id
             WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
             ORDER BY t.id"
        }
    };
    let mut stmt = conn.prepare(sql)?;
    Ok(stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn enrichment_stats(conn: &Connection) -> Result<EnrichmentStats> {
    let total_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks t
         WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))",
        [],
        |row| row.get(0),
    )?;
    let checked_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks t
         WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
           AND EXISTS (SELECT 1 FROM lastfm_checked lc WHERE lc.track_id = t.id)",
        [],
        |row| row.get(0),
    )?;
    let enriched_tracks: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks t
         WHERE (t.is_favorite = 1 OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
           AND (
               EXISTS (
                   SELECT 1 FROM track_genres tg
                   WHERE tg.track_id = t.id AND tg.source = 'lastfm'
               )
               OR EXISTS (
                   SELECT 1 FROM track_context_tags tct
                   WHERE tct.track_id = t.id AND tct.source = 'lastfm'
               )
           )",
        [],
        |row| row.get(0),
    )?;

    Ok(EnrichmentStats {
        total_tracks,
        checked_tracks,
        enriched_tracks,
        remaining_tracks: (total_tracks - checked_tracks).max(0),
    })
}

fn mark_checked(conn: &Connection, track_id: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO lastfm_checked (track_id, checked_at)
         VALUES (?1, datetime('now'))
         ON CONFLICT(track_id) DO UPDATE SET checked_at = excluded.checked_at",
        rusqlite::params![track_id],
    )?;
    Ok(())
}

fn should_replace_existing(
    mode: EnrichmentMode,
    transient_failure: bool,
    _has_replacement_rows: bool,
) -> bool {
    mode == EnrichmentMode::RefreshAll && !transient_failure
}

fn should_mark_track_checked(track_lookup_failed: bool, artist_lookup_failed: bool) -> bool {
    !track_lookup_failed && !artist_lookup_failed
}

fn context_confidence(count: Option<u32>) -> f64 {
    match count {
        Some(count) => ((count as f64).ln_1p() / (100f64).ln_1p())
            .min(1.0)
            .max(0.1),
        None => 0.5,
    }
}

pub(crate) fn route_tags(
    raw_tags: &[(String, Option<u32>, TagSource, TagLevel)],
    catalog: &GenreCatalog,
) -> (Vec<TagInput>, Vec<(String, String, TagContext, f64)>) {
    let mut genre_inputs = Vec::new();
    let mut context_rows = Vec::new();

    for (name, count, source, level) in raw_tags {
        let is_known = catalog.resolve_single(name).is_some();
        let classified = classify_tag_context(name, is_known);
        match classified.context {
            TagContext::Genre => genre_inputs.push(TagInput {
                name: name.clone(),
                source: *source,
                level: *level,
                count: *count,
            }),
            TagContext::Noise => {}
            context => context_rows.push((
                classified.raw,
                classified.normalized,
                context,
                context_confidence(*count),
            )),
        }
    }

    (genre_inputs, context_rows)
}

/// Run Last.fm tag enrichment over the user's favorited tracks.
///
/// Each raw Last.fm tag is classified before it can affect genres. True genre
/// tags are scored source-aware into `track_genres`; mood/vibe tags are kept in
/// `track_context_tags`; overloaded noise tags are discarded.
pub async fn run_enrichment<F, G>(
    state: SharedState,
    http: Client,
    api_key: String,
    mode: EnrichmentMode,
    cancel: Arc<AtomicBool>,
    mut artist_progress: G,
    mut progress: F,
) -> Result<()>
where
    F: FnMut(usize, usize) + Send + 'static,
    G: FnMut(usize, usize) + Send + 'static,
{
    info!("Last.fm enrichment started.");

    let client = LastFmClient::new(http, api_key);

    let tracks_to_enrich: Vec<(i64, String, String)> = state
        .read()
        .await
        .db
        .with_conn(|conn| load_tracks_to_enrich(conn, mode))?;

    let total = tracks_to_enrich.len();
    if total == 0 {
        info!("No tracks to enrich.");
        return Ok(());
    }

    let unique_artists: Vec<String> = tracks_to_enrich
        .iter()
        .map(|(_, _, artist)| artist.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let mut artist_cache: HashMap<String, Option<Vec<CountedTag>>> = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT artist_name, tags_json FROM lastfm_artist_cache")?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(|row| row.ok())
                .filter_map(|(name, json)| {
                    // Old cache rows were stored as Vec<String> with no counts. Those
                    // fail this deserialization, fall through as uncached, and are
                    // silently re-fetched once after deploy.
                    let tags: Vec<CountedTag> = serde_json::from_str(&json).ok()?;
                    Some((name, if tags.is_empty() { None } else { Some(tags) }))
                })
                .collect();
            Ok(rows)
        })
        .unwrap_or_default();

    let already_cached = artist_cache.len();
    let artist_total = unique_artists.len();
    let to_fetch: Vec<&String> = unique_artists
        .iter()
        .filter(|artist| !artist_cache.contains_key(*artist))
        .collect();
    let fetch_count = to_fetch.len();

    info!(
        "Last.fm artist pre-fetch: {} already cached, {} to fetch ({} unique artists, {} tracks).",
        already_cached, fetch_count, artist_total, total
    );

    artist_progress(already_cached, artist_total);

    let mut fetched_so_far = already_cached;
    let mut artist_transient_failures = HashSet::new();
    for artist in to_fetch {
        if cancel.load(Ordering::Relaxed) {
            info!("Last.fm enrichment cancelled during artist pre-fetch.");
            return Ok(());
        }

        let tags = match fetch_with_retry(|| client.artist_top_tags(artist)).await {
            Ok(tags) => tags,
            Err(()) => {
                artist_transient_failures.insert(artist.clone());
                fetched_so_far += 1;
                artist_progress(fetched_so_far, artist_total);
                if fetched_so_far.is_multiple_of(500) {
                    info!("Artist pre-fetch: {}/{}", fetched_so_far, artist_total);
                }
                sleep(Duration::from_millis(CALL_DELAY_MS)).await;
                continue;
            }
        };
        let tags_opt = if tags.is_empty() {
            None
        } else {
            Some(tags.clone())
        };
        let tags_json = serde_json::to_string(&tags).unwrap_or_else(|_| "[]".to_string());
        let _ = state.read().await.db.with_conn(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO lastfm_artist_cache (artist_name, tags_json) VALUES (?1, ?2)",
                rusqlite::params![artist, tags_json],
            )?;
            Ok(())
        });
        artist_cache.insert(artist.clone(), tags_opt);

        fetched_so_far += 1;
        artist_progress(fetched_so_far, artist_total);
        if fetched_so_far.is_multiple_of(500) {
            info!("Artist pre-fetch: {}/{}", fetched_so_far, artist_total);
        }
        sleep(Duration::from_millis(CALL_DELAY_MS)).await;
    }

    info!(
        "Artist pre-fetch complete ({} cached). Starting per-track pass.",
        artist_cache.len()
    );

    let mut processed = 0usize;
    let mut tagged = 0usize;
    let mut transient_skips = 0usize;

    for (track_id, title, artist) in tracks_to_enrich {
        let artist_lookup_failed = artist_transient_failures.contains(&artist);
        let mut track_lookup_failed = false;
        let track_tags = match fetch_with_retry(|| client.track_top_tags(&artist, &title)).await {
            Ok(tags) => tags,
            Err(()) => {
                track_lookup_failed = true;
                Vec::new()
            }
        };
        let transient_failure = track_lookup_failed || artist_lookup_failed;

        let track_tagged = !track_tags.is_empty()
            || matches!(artist_cache.get(&artist), Some(Some(tags)) if !tags.is_empty());

        let _ = state.read().await.db.with_conn(|conn| {
            let catalog = crate::genre::builder::embedded_builder().catalog();
            let mut routed_input: Vec<(String, Option<u32>, TagSource, TagLevel)> = Vec::new();
            let mut seen: HashSet<(String, TagSource, TagLevel)> = HashSet::new();

            for (name, count) in &track_tags {
                if is_artist_name_tag(name, conn) {
                    continue;
                }
                let key = (
                    name.to_ascii_lowercase(),
                    TagSource::LastFmTrack,
                    TagLevel::Recording,
                );
                if seen.insert(key) {
                    routed_input.push((
                        name.clone(),
                        *count,
                        TagSource::LastFmTrack,
                        TagLevel::Recording,
                    ));
                }
            }

            if let Some(Some(tags)) = artist_cache.get(&artist) {
                for (name, count) in tags {
                    if is_artist_name_tag(name, conn) {
                        continue;
                    }
                    let key = (
                        name.to_ascii_lowercase(),
                        TagSource::LastFmArtist,
                        TagLevel::Artist,
                    );
                    if seen.insert(key) {
                        routed_input.push((
                            name.clone(),
                            *count,
                            TagSource::LastFmArtist,
                            TagLevel::Artist,
                        ));
                    }
                }
            }

            let (genre_inputs, context_rows) = route_tags(&routed_input, catalog);
            let result = score_genre_tags(&genre_inputs, MIN_SCORE_FLOOR);
            let has_replacement_rows = !result.genres.is_empty() || !context_rows.is_empty();

            if should_replace_existing(mode, transient_failure, has_replacement_rows) {
                conn.execute(
                    "DELETE FROM track_genres WHERE track_id = ?1 AND source = 'lastfm'",
                    rusqlite::params![track_id],
                )?;
                conn.execute(
                    "DELETE FROM track_context_tags WHERE track_id = ?1 AND source = 'lastfm'",
                    rusqlite::params![track_id],
                )?;
            }

            for scored in &result.genres {
                let Some(genre_id): Option<i64> = conn
                    .query_row(
                        "SELECT id FROM genres WHERE name = ?1",
                        [&scored.canonical],
                        |row| row.get(0),
                    )
                    .ok()
                else {
                    continue;
                };

                conn.execute(
                    "INSERT INTO track_genres (track_id, genre_id, source, confidence)
                     VALUES (?1, ?2, 'lastfm', ?3)
                     ON CONFLICT(track_id, genre_id) DO UPDATE SET
                         confidence = MAX(confidence, excluded.confidence)",
                    rusqlite::params![track_id, genre_id, scored.score],
                )?;
            }

            for (raw, normalized, context, confidence) in &context_rows {
                conn.execute(
                    "INSERT INTO track_context_tags
                         (track_id, tag, normalized_tag, context, source, confidence)
                     VALUES (?1, ?2, ?3, ?4, 'lastfm', ?5)
                     ON CONFLICT(track_id, normalized_tag, context, source) DO UPDATE SET
                         confidence = MAX(confidence, excluded.confidence)",
                    rusqlite::params![track_id, raw, normalized, context.as_str(), confidence],
                )?;
            }

            if should_mark_track_checked(track_lookup_failed, artist_lookup_failed) {
                mark_checked(conn, track_id)?;
            }
            Ok(())
        });

        if track_tagged {
            tagged += 1;
        }
        if transient_failure {
            transient_skips += 1;
        }

        processed += 1;
        progress(processed, total);

        if processed.is_multiple_of(500) {
            info!(
                "Last.fm enrichment: {}/{} processed ({} tagged, {} retry-later)",
                processed, total, tagged, transient_skips
            );
        }

        if cancel.load(Ordering::Relaxed) {
            info!(
                "Last.fm enrichment stopped by user after {} tracks.",
                processed
            );
            return Ok(());
        }

        sleep(Duration::from_millis(CALL_DELAY_MS)).await;
    }

    info!(
        "Last.fm enrichment complete. Processed {} tracks ({} tagged, {} retry-later).",
        processed, tagged, transient_skips
    );
    Ok(())
}

async fn fetch_with_retry<T, F, Fut>(mut f: F) -> Result<Vec<T>, ()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Vec<T>>>,
{
    for attempt in 0..MAX_RETRIES {
        match f().await {
            Ok(tags) => return Ok(tags),
            Err(error) => {
                let msg = format!("{}", error);
                if msg.contains("not found") || msg.contains("status 6") {
                    return Ok(Vec::new());
                }
                warn!(
                    "Last.fm error (attempt {}/{}): {}",
                    attempt + 1,
                    MAX_RETRIES,
                    msg
                );
                sleep(Duration::from_secs(2u64.pow(attempt))).await;
            }
        }
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn lastfm_test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE albums (id INTEGER PRIMARY KEY, is_favorite INTEGER DEFAULT 0);
            CREATE TABLE tracks (
                id INTEGER PRIMARY KEY,
                title TEXT NOT NULL,
                artist_id INTEGER NOT NULL,
                album_id INTEGER,
                is_favorite INTEGER DEFAULT 0
            );
            CREATE TABLE lastfm_checked (
                track_id INTEGER PRIMARY KEY,
                checked_at TEXT DEFAULT (datetime('now'))
            );
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn mood_tags_are_routed_to_context_not_genres() {
        let catalog = crate::genre::builder::embedded_builder().catalog();
        let inputs = vec![
            (
                "happy".into(),
                Some(80),
                TagSource::LastFmTrack,
                TagLevel::Recording,
            ),
            (
                "birthday".into(),
                Some(40),
                TagSource::LastFmTrack,
                TagLevel::Recording,
            ),
            (
                "techno".into(),
                Some(60),
                TagSource::LastFmTrack,
                TagLevel::Recording,
            ),
        ];

        let (genre_inputs, context_rows) = route_tags(&inputs, catalog);

        assert_eq!(genre_inputs.len(), 1);
        assert_eq!(genre_inputs[0].name, "techno");

        let context_names: Vec<&str> = context_rows
            .iter()
            .map(|(_, normalized, _, _)| normalized.as_str())
            .collect();
        assert!(context_names.contains(&"happy"));
        assert!(context_names.contains(&"birthday"));
    }

    #[test]
    fn missing_count_uses_neutral_context_confidence() {
        let catalog = crate::genre::builder::embedded_builder().catalog();
        let inputs = vec![(
            "happy".into(),
            None,
            TagSource::LastFmTrack,
            TagLevel::Recording,
        )];
        let (_, context_rows) = route_tags(&inputs, catalog);
        assert_eq!(context_rows[0].3, 0.5);
    }

    #[test]
    fn old_artist_cache_shape_falls_through_to_refetch() {
        let old_json = serde_json::to_string(&vec!["rock".to_string()]).unwrap();
        let parsed: Option<Vec<CountedTag>> = serde_json::from_str(&old_json).ok();
        assert!(parsed.is_none());
    }

    #[test]
    fn refresh_mode_selects_already_checked_eligible_tracks() {
        let conn = lastfm_test_conn();
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Autechre')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO albums (id, is_favorite) VALUES (10, 1), (11, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, is_favorite) VALUES
                (1, 'Flutter', 1, 11, 1),
                (2, 'Bike', 1, 10, 0),
                (3, 'Basscadet', 1, 11, 0)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO lastfm_checked (track_id) VALUES (1), (2)", [])
            .unwrap();

        assert_eq!(
            count_tracks_to_enrich(&conn, EnrichmentMode::Pending).unwrap(),
            0
        );
        assert_eq!(
            count_tracks_to_enrich(&conn, EnrichmentMode::RefreshAll).unwrap(),
            2
        );

        let refresh_tracks = load_tracks_to_enrich(&conn, EnrichmentMode::RefreshAll).unwrap();
        let ids = refresh_tracks
            .iter()
            .map(|(id, _, _)| *id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![1, 2]);
    }

    #[test]
    fn retry_untagged_mode_selects_checked_tracks_without_lastfm_outputs() {
        let conn = lastfm_test_conn();
        conn.execute("INSERT INTO artists (id, name) VALUES (1, 'Autechre')", [])
            .unwrap();
        conn.execute("INSERT INTO albums (id, is_favorite) VALUES (10, 1)", [])
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE genres (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             CREATE TABLE track_genres (
                track_id INTEGER NOT NULL,
                genre_id INTEGER NOT NULL,
                source TEXT NOT NULL,
                confidence REAL DEFAULT 1.0,
                PRIMARY KEY (track_id, genre_id)
             );
             CREATE TABLE track_context_tags (
                track_id INTEGER NOT NULL,
                tag TEXT NOT NULL,
                normalized_tag TEXT NOT NULL,
                context TEXT NOT NULL,
                source TEXT NOT NULL,
                confidence REAL NOT NULL DEFAULT 0.5,
                PRIMARY KEY (track_id, normalized_tag, context, source)
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tracks (id, title, artist_id, album_id, is_favorite) VALUES
                (1, 'Flutter', 1, 10, 0),
                (2, 'Bike', 1, 10, 0),
                (3, 'Basscadet', 1, 10, 0),
                (4, 'Pir', 1, 10, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lastfm_checked (track_id) VALUES (1), (2), (3)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO genres (id, name) VALUES (1, 'Techno')", [])
            .unwrap();
        conn.execute(
            "INSERT INTO track_genres (track_id, genre_id, source) VALUES (2, 1, 'lastfm')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO track_context_tags
                (track_id, tag, normalized_tag, context, source, confidence)
             VALUES (3, 'happy', 'happy', 'mood', 'lastfm', 0.7)",
            [],
        )
        .unwrap();

        assert_eq!(
            count_tracks_to_enrich(&conn, EnrichmentMode::RetryUntagged).unwrap(),
            1
        );

        let retry_tracks = load_tracks_to_enrich(&conn, EnrichmentMode::RetryUntagged).unwrap();
        let ids = retry_tracks
            .iter()
            .map(|(id, _, _)| *id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn refresh_replaces_existing_rows_on_successful_empty_results() {
        assert!(should_replace_existing(
            EnrichmentMode::RefreshAll,
            false,
            false
        ));
        assert!(should_replace_existing(
            EnrichmentMode::RefreshAll,
            false,
            true
        ));
        assert!(!should_replace_existing(
            EnrichmentMode::RefreshAll,
            true,
            true
        ));
        assert!(!should_replace_existing(
            EnrichmentMode::Pending,
            false,
            true
        ));
    }

    #[test]
    fn track_is_not_checked_when_artist_lookup_had_transient_failure() {
        assert!(should_mark_track_checked(false, false));
        assert!(!should_mark_track_checked(true, false));
        assert!(!should_mark_track_checked(false, true));
    }

    #[tokio::test]
    async fn fetch_with_retry_treats_lastfm_not_found_as_empty() {
        let rows: Vec<CountedTag> = fetch_with_retry(|| async {
            Err(anyhow::anyhow!(
                "Last.fm API error status 6: Track not found"
            ))
        })
        .await
        .expect("Last.fm not-found errors should be definitive empty results");

        assert!(rows.is_empty());
    }
}
