use anyhow::Result;
use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::time::{sleep, Duration};
use tracing::{info, warn};

use crate::metadata::lastfm::LastFmClient;
use crate::SharedState;

// 200ms per API call keeps us at ~5 req/sec (Last.fm's documented limit).
// With artist tags pre-fetched, each track only needs one call.
const CALL_DELAY_MS: u64 = 200;
const MAX_RETRIES: u32 = 3;

// Common Last.fm tags that aren't genres. Filtered before auto-grow so we
// don't pollute the genre catalog with "seen live" / "favourites" / decade
// markers. Anything not in this set is still subject to the canonical genre
// catalog resolution before being stored.
const NON_GENRE_TAGS: &[&str] = &[
    "seen live",
    "seen-live",
    "favourite",
    "favourites",
    "favorite",
    "favorites",
    "love",
    "loved",
    "owned",
    "albums i own",
    "albums-i-own",
    "to listen to",
    "to-listen-to",
    "to check out",
    "rip",
    "amazing",
    "awesome",
    "cool",
    "good",
    "great",
    "best",
    "00s",
    "10s",
    "20s",
    "60s",
    "70s",
    "80s",
    "90s",
    "english",
    "spanish",
    "french",
    "german",
    "japanese",
    "male vocalists",
    "female vocalists",
    "male vocalist",
    "female vocalist",
    "vocalists",
    "instrumental",
];

fn slugify(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    let mut prev_dash = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            s.extend(ch.to_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            s.push('-');
            prev_dash = true;
        }
    }
    if s.ends_with('-') {
        s.pop();
    }
    if s.is_empty() {
        s.push_str("genre");
    }
    s
}

fn title_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut at_word_start = true;
    for ch in name.chars() {
        if ch.is_whitespace() || ch == '-' {
            out.push(ch);
            at_word_start = true;
        } else if at_word_start {
            out.extend(ch.to_uppercase());
            at_word_start = false;
        } else {
            out.extend(ch.to_lowercase());
        }
    }
    out
}

fn is_non_genre(tag: &str) -> bool {
    let lower = tag.trim().to_ascii_lowercase();
    NON_GENRE_TAGS.iter().any(|&banned| banned == lower)
}

fn canonicalize_or_passthrough(raw: &str) -> Option<(String, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_non_genre(trimmed) {
        return None;
    }
    let resolution = crate::genre::builder::embedded_builder().resolve(trimmed);
    let name = resolution
        .canonical_name()
        .map(str::to_string)
        .unwrap_or_else(|| title_case(trimmed));
    let slug = slugify(&name);
    Some((name, slug))
}

/// Run Last.fm tag enrichment over the user's favorited tracks.
///
/// For each track: fetch top tags from `track.gettoptags`, then top tags from
/// `artist.gettoptags`, combine, filter junk, canonicalize through NOOR's
/// genre catalog (with auto-grow fallback), and persist.
///
/// Mirrors the Spotify enrichment design: track only stamped into
/// `lastfm_checked` when no transient failure occurred, so retries on next
/// run are possible. Genres we *did* fetch are always persisted.
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
    // Artist tags are persisted to lastfm_artist_cache so the prefetch phase
    // survives server restarts. Only artists absent from that table are fetched
    // over the network; the rest are loaded instantly from DB.
    let unique_artists: Vec<String> = tracks_to_enrich
        .iter()
        .map(|(_, _, a)| a.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    // Load whatever was already persisted from a previous run.
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
        "Last.fm artist pre-fetch: {} already cached, {} to fetch (total {} unique artists, {} tracks).",
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
        // Persist immediately so a restart can resume.
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
        "Artist pre-fetch complete ({} total cached). Starting per-track pass.",
        artist_cache.len()
    );

    // ── Phase 2: process each track (one API call each) ───────────────────────
    let mut processed = 0usize;
    let mut tagged = 0usize;
    let mut transient_skips = 0usize;

    for (track_id, title, artist) in tracks_to_enrich {
        let mut raw_tags: HashSet<String> = HashSet::new();
        let mut transient_failure = false;

        match fetch_with_retry(|| client.track_top_tags(&artist, &title)).await {
            Ok(tags) => {
                for t in tags {
                    raw_tags.insert(t);
                }
            }
            Err(()) => transient_failure = true,
        }

        if let Some(Some(tags)) = artist_cache.get(&artist) {
            for t in tags {
                raw_tags.insert(t.clone());
            }
        }

        let track_tagged = !raw_tags.is_empty();

        let _ = state.read().await.db.with_conn(|conn| {
            for raw in &raw_tags {
                let Some((name, slug)) = canonicalize_or_passthrough(raw) else {
                    continue;
                };
                conn.execute(
                    "INSERT OR IGNORE INTO genres (name, slug, parent_id) VALUES (?1, ?2, NULL)",
                    rusqlite::params![name, slug],
                )?;
                let genre_id: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM genres WHERE name = ?1",
                        [&name],
                        |row| row.get(0),
                    )
                    .ok();
                if let Some(id) = genre_id {
                    conn.execute(
                        "INSERT OR IGNORE INTO track_genres (track_id, genre_id, source, confidence) VALUES (?1, ?2, 'lastfm', 0.7)",
                        rusqlite::params![track_id, id],
                    )?;
                }
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

// LastFmClient::get_json bails on any non-2xx status. We retry on any error
// up to MAX_RETRIES with exponential backoff, then give up. Last.fm's 429
// header behavior is inconsistent so we don't try to read Retry-After here —
// just back off generously.
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
                // Don't retry definitive misses; Last.fm returns these as
                // status 6 / "track not found" wrapped in 200, so the err
                // text is the only signal.
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
