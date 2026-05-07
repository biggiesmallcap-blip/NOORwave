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

const CALL_DELAY_MS: u64 = 200;
const MAX_RETRIES: u32 = 3;

type CountedTag = (String, Option<u32>);

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

    let tracks_to_enrich: Vec<(i64, String, String)> = state.read().await.db.with_conn(|conn| {
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
            .filter_map(|row| row.ok())
            .collect::<Vec<_>>())
    })?;

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
    for artist in to_fetch {
        if cancel.load(Ordering::Relaxed) {
            info!("Last.fm enrichment cancelled during artist pre-fetch.");
            return Ok(());
        }

        let tags = match fetch_with_retry(|| client.artist_top_tags(artist)).await {
            Ok(tags) => tags,
            Err(()) => Vec::new(),
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
        let mut transient_failure = false;
        let track_tags = match fetch_with_retry(|| client.track_top_tags(&artist, &title)).await {
            Ok(tags) => tags,
            Err(()) => {
                transient_failure = true;
                Vec::new()
            }
        };

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
}
