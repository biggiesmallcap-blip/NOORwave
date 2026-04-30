use anyhow::Result;
use reqwest::Client;
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

use crate::metadata::lastfm::LastFmClient;
use crate::services::lastfm::tag_filter::should_keep_tag;
use crate::SharedState;

// 200ms per API call keeps us at ~5 req/sec (Last.fm's documented limit).
// With artist tags pre-fetched, each track only needs one call.
const CALL_DELAY_MS: u64 = 200;
const MAX_RETRIES: u32 = 3;

// Top N tags to consider per track. Tags come from Last.fm sorted by vote
// count descending, so slicing here keeps only the most-agreed-upon ones.
const MAX_TAGS_PER_TRACK: usize = 5;

// Confidence for Last.fm-sourced genres. Lower than MB (0.90) and Tidal
// (0.85) to reflect the crowd-voted, noisy nature of the source.
const LASTFM_CONFIDENCE: f64 = 0.40;

/// Resolve a raw Last.fm tag to a pre-existing genre ID in the closed taxonomy.
///
/// Returns `None` if the tag fails the pre-filter, has no canonical match, or
/// doesn't correspond to an already-seeded genre row. The genres table is a
/// closed set — this function never inserts into it.
///
/// Tags that pass the keep-filter but fail canonical resolution are logged
/// to `lastfm_unresolved_tags` for taxonomy-curation triage. See
/// docs/tidal-genre-source-investigation.md (appendix) for why this is the
/// right log point.
fn resolve_to_genre_id(tag: &str, track_id: i64, conn: &Connection) -> Option<i64> {
    if !should_keep_tag(tag, conn) {
        return None;
    }

    let resolution = crate::genre::builder::embedded_builder().resolve(tag);
    let Some(canonical) = resolution.canonical_name() else {
        let _ = conn.execute(
            "INSERT INTO lastfm_unresolved_tags (tag, seen_count, last_seen, last_track_id)
             VALUES (?1, 1, datetime('now'), ?2)
             ON CONFLICT(tag) DO UPDATE SET
                 seen_count    = seen_count + 1,
                 last_seen     = datetime('now'),
                 last_track_id = excluded.last_track_id",
            rusqlite::params![tag, track_id],
        );
        return None;
    };

    conn.query_row(
        "SELECT id FROM genres WHERE name = ?1",
        [canonical],
        |row| row.get::<_, i64>(0),
    )
    .ok()
}

/// Build the set of genre IDs already associated with a track and a map from
/// each genre_id to its parent_id for hierarchy comparisons.
fn existing_genre_ids(
    track_id: i64,
    conn: &Connection,
) -> (HashSet<i64>, HashMap<i64, Option<i64>>) {
    let rows: Vec<(i64, Option<i64>)> = {
        let mut stmt = conn
            .prepare(
                "SELECT g.id, g.parent_id
                 FROM track_genres tg
                 JOIN genres g ON g.id = tg.genre_id
                 WHERE tg.track_id = ?1",
            )
            .ok();
        stmt.as_mut()
            .map(|s| {
                s.query_map([track_id], |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap_or_else(|_| panic!("query failed"))
                    .filter_map(|r| r.ok())
                    .collect()
            })
            .unwrap_or_default()
    };

    let ids: HashSet<i64> = rows.iter().map(|(id, _)| *id).collect();
    let parents: HashMap<i64, Option<i64>> = rows.into_iter().collect();
    (ids, parents)
}

/// Decide whether to insert a new genre association for a track.
///
/// Rules (from the Codex review):
/// - Drop if the candidate is a direct parent of an already-associated genre
///   (less specific — adds noise).
/// - Keep if the candidate is a direct child of an already-associated genre
///   (more specific — valuable refinement, e.g. Tidal says "Electronic",
///   Last.fm says "Drum and Bass" → keep).
/// - Keep if completely new (no overlap with existing associations).
fn should_insert(
    candidate_id: i64,
    _candidate_parent_id: Option<i64>,
    existing_ids: &HashSet<i64>,
    existing_parents: &HashMap<i64, Option<i64>>,
) -> bool {
    if existing_ids.contains(&candidate_id) {
        return false; // already associated — handled by upsert below
    }

    // Drop if this candidate is a parent of any already-stored genre.
    for (_existing_id, existing_parent) in existing_parents {
        if *existing_parent == Some(candidate_id) {
            // candidate IS the parent of an existing genre → less specific → drop
            return false;
        }
    }

    // The candidate is either a child of an existing genre or entirely new — keep.
    true
}

/// Look up the parent_id of a genre by its id.
fn genre_parent_id(genre_id: i64, conn: &Connection) -> Option<i64> {
    conn.query_row(
        "SELECT parent_id FROM genres WHERE id = ?1",
        [genre_id],
        |row| row.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
}

/// Run Last.fm tag enrichment over the user's favorited tracks.
///
/// For each track: fetch top tags from `track.gettoptags`, then merge with
/// cached artist tags, filter via `tag_filter`, resolve through the closed
/// genre taxonomy, apply hierarchy-aware deduplication, and persist.
///
/// Genres are never inserted here — the genres table is a closed ontology
/// seeded from the taxonomy. Only `track_genres` is written to.
pub async fn run_enrichment<F, G>(
    state: SharedState,
    http: Client,
    api_key: String,
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
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.id, t.title, a.name
                 FROM tracks t
                 JOIN artists a ON t.artist_id = a.id
                 WHERE (t.is_favorite = 1
                        OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
                   AND NOT EXISTS (
                       SELECT 1 FROM lastfm_checked lc WHERE lc.track_id = t.id
                   )",
            )?;
            Ok(stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>())
        })?;

    let total = tracks_to_enrich.len();
    if total == 0 {
        info!("No tracks to enrich.");
        return Ok(());
    }

    // ── Phase 1: pre-fetch all unique artist tags ─────────────────────────────
    let unique_artists: Vec<String> = tracks_to_enrich
        .iter()
        .map(|(_, _, a)| a.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let mut artist_cache: HashMap<String, Option<Vec<String>>> = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT artist_name, tags_json FROM lastfm_artist_cache",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(|r| r.ok())
                .filter_map(|(name, json)| {
                    let tags: Vec<String> = serde_json::from_str(&json).ok()?;
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
        .filter(|a| !artist_cache.contains_key(*a))
        .collect();
    let fetch_count = to_fetch.len();

    info!(
        "Last.fm artist pre-fetch: {} already cached, {} to fetch ({} unique artists, {} tracks).",
        already_cached, fetch_count, artist_total, total
    );

    artist_progress(already_cached, artist_total);

    let mut fetched_so_far = already_cached;
    for artist in to_fetch {
        if cancel.load(Ordering::Relaxed) {
            info!("Last.fm enrichment cancelled during artist pre-fetch.");
            return Ok(());
        }
        let tags = match fetch_with_retry(|| client.artist_top_tags(artist)).await {
            Ok(t) => t,
            Err(()) => vec![],
        };
        let tags_opt: Option<Vec<String>> = if tags.is_empty() { None } else { Some(tags.clone()) };
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
        if fetched_so_far % 500 == 0 {
            info!("Artist pre-fetch: {}/{}", fetched_so_far, artist_total);
        }
        sleep(Duration::from_millis(CALL_DELAY_MS)).await;
    }
    info!(
        "Artist pre-fetch complete ({} cached). Starting per-track pass.",
        artist_cache.len()
    );

    // ── Phase 2: process each track (one API call each) ───────────────────────
    let mut processed = 0usize;
    let mut tagged = 0usize;
    let mut transient_skips = 0usize;

    for (track_id, title, artist) in tracks_to_enrich {
        // Collect tags in order (Last.fm returns by vote count desc). Use a
        // seen set to deduplicate while preserving insertion order.
        let mut raw_tags: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut transient_failure = false;

        match fetch_with_retry(|| client.track_top_tags(&artist, &title)).await {
            Ok(tags) => {
                for t in tags {
                    let key = t.to_ascii_lowercase();
                    if seen.insert(key) {
                        raw_tags.push(t);
                    }
                }
            }
            Err(()) => transient_failure = true,
        }

        if let Some(Some(tags)) = artist_cache.get(&artist) {
            for t in tags {
                let key = t.to_ascii_lowercase();
                if seen.insert(key) {
                    raw_tags.push(t.clone());
                }
            }
        }

        // Limit to the top N most popular tags.
        raw_tags.truncate(MAX_TAGS_PER_TRACK);

        let track_tagged = !raw_tags.is_empty();

        let _ = state.read().await.db.with_conn(|conn| {
            let (existing_ids, existing_parents) = existing_genre_ids(track_id, conn);

            for raw in &raw_tags {
                let Some(genre_id) = resolve_to_genre_id(raw, track_id, conn) else {
                    continue;
                };

                let candidate_parent = genre_parent_id(genre_id, conn);

                if !should_insert(genre_id, candidate_parent, &existing_ids, &existing_parents) {
                    continue;
                }

                // Upsert: if genre already exists from a higher-confidence source,
                // keep the higher confidence; otherwise insert at Last.fm confidence.
                conn.execute(
                    "INSERT INTO track_genres (track_id, genre_id, source, confidence)
                     VALUES (?1, ?2, 'lastfm', ?3)
                     ON CONFLICT(track_id, genre_id) DO UPDATE SET
                         confidence = MAX(confidence, excluded.confidence)",
                    rusqlite::params![track_id, genre_id, LASTFM_CONFIDENCE],
                )?;
            }

            if !transient_failure {
                conn.execute(
                    "INSERT OR IGNORE INTO lastfm_checked (track_id) VALUES (?1)",
                    rusqlite::params![track_id],
                )?;
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

        if processed % 500 == 0 {
            info!(
                "Last.fm enrichment: {}/{} processed ({} tagged, {} retry-later)",
                processed, total, tagged, transient_skips
            );
        }

        if cancel.load(Ordering::Relaxed) {
            info!("Last.fm enrichment stopped by user after {} tracks.", processed);
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

async fn fetch_with_retry<F, Fut>(mut f: F) -> Result<Vec<String>, ()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Vec<String>>>,
{
    for attempt in 0..MAX_RETRIES {
        match f().await {
            Ok(tags) => return Ok(tags),
            Err(e) => {
                let msg = format!("{}", e);
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

    fn setup_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             CREATE TABLE genres (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             CREATE TABLE lastfm_unresolved_tags (
                 tag           TEXT PRIMARY KEY,
                 seen_count    INTEGER NOT NULL DEFAULT 1,
                 last_seen     TEXT NOT NULL DEFAULT (datetime('now')),
                 last_track_id INTEGER
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn unresolvable_tag_gets_logged_with_track_context() {
        let conn = setup_conn();

        // Nonsense string: passes the keep-filter (length, not a stop tag, not
        // a locale, not a digit-decade, not a known artist) but the embedded
        // genre resolver returns no canonical match (jaro_winkler well below
        // the 0.92 fuzzy threshold and no shared tokens with any taxonomy
        // node). This is the previously-silent drop path.
        let raw = "zzzfakegenrexyz";

        let result = resolve_to_genre_id(raw, 42, &conn);
        assert!(result.is_none(), "expected unresolvable tag to return None");

        let (logged_tag, seen_count, last_track_id): (String, i64, Option<i64>) = conn
            .query_row(
                "SELECT tag, seen_count, last_track_id
                 FROM lastfm_unresolved_tags WHERE tag = ?1",
                [raw],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("unresolved tag row should have been inserted");

        assert_eq!(logged_tag, raw);
        assert_eq!(seen_count, 1);
        assert_eq!(last_track_id, Some(42));
    }

    #[test]
    fn repeat_observation_increments_count_and_updates_track_id() {
        let conn = setup_conn();
        let raw = "zzzfakegenrexyz";

        resolve_to_genre_id(raw, 42, &conn);
        resolve_to_genre_id(raw, 99, &conn);

        let (seen_count, last_track_id): (i64, Option<i64>) = conn
            .query_row(
                "SELECT seen_count, last_track_id
                 FROM lastfm_unresolved_tags WHERE tag = ?1",
                [raw],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();

        assert_eq!(seen_count, 2);
        assert_eq!(last_track_id, Some(99));
    }

    #[test]
    fn filtered_tag_does_not_get_logged() {
        let conn = setup_conn();

        // Stop tags are intentional non-genres; they should not pollute the
        // unresolved log because they're not candidates for taxonomy expansion.
        resolve_to_genre_id("seen live", 42, &conn);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM lastfm_unresolved_tags",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
