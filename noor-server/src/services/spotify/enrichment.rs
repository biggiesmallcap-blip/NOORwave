use anyhow::Result;
use reqwest::{Client, Response, StatusCode};
use serde::Deserialize;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use crate::SharedState;
use crate::services::spotify::auth::SpotifyTokenMode;

const SPOTIFY_SEARCH_URL: &str = "https://api.spotify.com/v1/search";
const SPOTIFY_ALBUM_URL: &str = "https://api.spotify.com/v1/albums/{}";
const SPOTIFY_ARTISTS_URL: &str = "https://api.spotify.com/v1/artists";

// Per-track delay, calibrated by token source. Three API calls per track
// (search + album + artists batch).
//
// Client-creds path: 600ms/track → ~5 req/sec, matching the Last.fm enricher's
// outbound rate (200ms × 1 call/track). Spotify's documented limit is ~180
// req/min for app tokens; we stay well under it.
//
// Anonymous path: 1000ms/track → ~3 req/sec. The endpoint is undocumented and
// we don't want to be the user that gets it noticed. Conservative is the right
// default here; a user with their own client creds gets the faster path.
const PER_TRACK_DELAY_CLIENT_CREDS_MS: u64 = 600;
const PER_TRACK_DELAY_ANONYMOUS_MS: u64 = 1000;
const MAX_RETRIES: u32 = 4;

fn delay_for(mode: SpotifyTokenMode) -> u64 {
    match mode {
        SpotifyTokenMode::ClientCredentials => PER_TRACK_DELAY_CLIENT_CREDS_MS,
        SpotifyTokenMode::Anonymous => PER_TRACK_DELAY_ANONYMOUS_MS,
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    tracks: TrackPage,
}

#[derive(Debug, Deserialize)]
struct TrackPage {
    items: Vec<SpotifyTrack>,
}

#[derive(Debug, Deserialize)]
struct SpotifyTrack {
    #[allow(dead_code)]
    id: String,
    album: AlbumRef,
}

#[derive(Debug, Deserialize)]
struct AlbumRef {
    id: String,
}

#[derive(Debug, Deserialize)]
struct AlbumDetails {
    genres: Vec<String>,
    artists: Vec<ArtistIdRef>,
}

#[derive(Debug, Deserialize)]
struct ArtistIdRef {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ArtistsBatchResponse {
    artists: Vec<ArtistDetails>,
}

#[derive(Debug, Deserialize)]
struct ArtistDetails {
    #[serde(default)]
    genres: Vec<String>,
}

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

// Resolve via NOOR's curated catalog first ("trip hop" → "Trip-Hop"); fall back
// to title-casing the raw Spotify tag so micro-genres like "alt z" or
// "vapor twitch" can still be recorded as new genres instead of being dropped.
fn canonicalize_or_passthrough(raw: &str) -> Option<(String, String)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
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

/// Outcome of a single Spotify GET.
/// - Ok(Some(resp)) — 200, here's the body
/// - Ok(None)       — definitive no-data (404), don't retry
/// - Err(())        — transient failure (429/5xx/network/auth) after retries.
///                    Caller should NOT mark the track as `spotify_checked` so
///                    we get another shot on the next run.
async fn spotify_get(http: &Client, url: &str, token: &str) -> Result<Option<Response>, ()> {
    for attempt in 0..MAX_RETRIES {
        let result = http
            .get(url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await;
        let resp = match result {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    "Spotify network error (attempt {}/{}): {}",
                    attempt + 1,
                    MAX_RETRIES,
                    e
                );
                sleep(Duration::from_secs(2u64.pow(attempt))).await;
                continue;
            }
        };
        let status = resp.status();
        if status.is_success() {
            return Ok(Some(resp));
        }
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(5)
                .clamp(1, 60);
            warn!(
                "Spotify 429 — backing off {}s (attempt {}/{})",
                retry_after,
                attempt + 1,
                MAX_RETRIES
            );
            sleep(Duration::from_secs(retry_after)).await;
            continue;
        }
        if status.is_server_error() {
            warn!(
                "Spotify {} — retrying (attempt {}/{})",
                status,
                attempt + 1,
                MAX_RETRIES
            );
            sleep(Duration::from_secs(2u64.pow(attempt))).await;
            continue;
        }
        // Other 4xx (401, 403, 400). 401 likely means stale token — bail
        // transient so the caller skips this track and ensure_token gets
        // another chance next iteration.
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(300).collect();
        warn!("Spotify {} for {} — body: {}", status, url, snippet);
        return Err(());
    }
    Err(())
}

/// Run Spotify genre enrichment.
///
/// For every eligible track: search → fetch album → fetch its artists.
/// Most genre data lives on artist objects, not albums, so artists are the
/// primary source. A track is recorded in `spotify_checked` only when every
/// API call we made for it succeeded (or returned a definitive no-data) — if
/// we hit a transient failure (429s after retries, 5xx, network error), the
/// row is left unmarked so the next run tries again. Genres we *did* manage
/// to fetch in a partial run are still persisted so the work isn't lost.
pub async fn run_enrichment<F>(state: SharedState, http: Client, mut progress: F) -> Result<()>
where
    F: FnMut(usize, usize) + Send + 'static,
{
    info!("Spotify enrichment started.");

    let tracks_to_enrich: Vec<(i64, String, String, Option<String>)> =
        state.read().await.db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT t.id, t.title, a.name, t.isrc
                 FROM tracks t
                 JOIN artists a ON t.artist_id = a.id
                 WHERE (t.is_favorite = 1
                        OR t.album_id IN (SELECT id FROM albums WHERE is_favorite = 1))
                   AND NOT EXISTS (
                       SELECT 1 FROM spotify_checked sc WHERE sc.track_id = t.id
                   )",
            )?;
            Ok(stmt
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })?
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>())
        })?;

    let total = tracks_to_enrich.len();
    if total == 0 {
        info!("No tracks to enrich.");
        return Ok(());
    }

    let mut processed = 0usize;
    let mut tagged = 0usize;
    let mut transient_skips = 0usize;

    for (track_id, title, artist, isrc) in tracks_to_enrich {
        let (token, token_mode) = match ensure_token(&state, &http).await {
            Some(t) => t,
            None => break,
        };

        let mut raw_genres: Vec<String> = Vec::new();
        let mut artist_ids: Vec<String> = Vec::new();
        let mut transient_failure = false;

        let query = if let Some(ref code) = isrc {
            format!("isrc:{}", code)
        } else {
            format!("artist:{} track:{}", artist, title)
        };
        let search_url = format!(
            "{}?q={}&type=track&limit=1",
            SPOTIFY_SEARCH_URL,
            urlencoding::encode(&query)
        );

        let track_album: Option<AlbumRef> = match spotify_get(&http, &search_url, &token).await {
            Ok(Some(resp)) => match resp.json::<SearchResponse>().await {
                Ok(d) => d.tracks.items.into_iter().next().map(|t| t.album),
                Err(e) => {
                    warn!("Spotify search JSON parse: {}", e);
                    None
                }
            },
            Ok(None) => None,
            Err(()) => {
                transient_failure = true;
                None
            }
        };

        if let Some(album_ref) = track_album {
            let album_url = SPOTIFY_ALBUM_URL.replace("{}", &album_ref.id);
            match spotify_get(&http, &album_url, &token).await {
                Ok(Some(resp)) => match resp.json::<AlbumDetails>().await {
                    Ok(album) => {
                        raw_genres.extend(album.genres);
                        artist_ids.extend(album.artists.into_iter().map(|a| a.id));
                    }
                    Err(e) => warn!("Spotify album JSON parse: {}", e),
                },
                Ok(None) => {}
                Err(()) => transient_failure = true,
            }
        }

        // Spotify allows up to 50 IDs per /v1/artists batch. A single track
        // rarely has more than a handful, so this is almost always one call.
        for chunk in artist_ids.chunks(50) {
            let ids = chunk.join(",");
            let url = format!("{}?ids={}", SPOTIFY_ARTISTS_URL, ids);
            match spotify_get(&http, &url, &token).await {
                Ok(Some(resp)) => match resp.json::<ArtistsBatchResponse>().await {
                    Ok(batch) => {
                        for art in batch.artists {
                            raw_genres.extend(art.genres);
                        }
                    }
                    Err(e) => warn!("Spotify artists JSON parse: {}", e),
                },
                Ok(None) => {}
                Err(()) => transient_failure = true,
            }
        }

        let track_tagged = !raw_genres.is_empty();

        // Persist whatever genres we did get. INSERT OR IGNORE means a future
        // re-run won't duplicate rows. Only stamp `spotify_checked` if the
        // whole exchange succeeded — otherwise leave the track for retry.
        let _ = state.read().await.db.with_conn(|conn| {
            for raw in &raw_genres {
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
                        "INSERT OR IGNORE INTO track_genres (track_id, genre_id, source, confidence) VALUES (?1, ?2, 'spotify', 1.0)",
                        rusqlite::params![track_id, id],
                    )?;
                }
            }
            if !transient_failure {
                conn.execute(
                    "INSERT OR IGNORE INTO spotify_checked (track_id) VALUES (?1)",
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

        if processed % 200 == 0 {
            info!(
                "Spotify enrichment: {}/{} processed ({} tagged, {} retry-later)",
                processed, total, tagged, transient_skips
            );
        }

        sleep(Duration::from_millis(delay_for(token_mode))).await;
    }

    info!(
        "Spotify enrichment complete. Processed {} tracks ({} tagged, {} retry-later).",
        processed, tagged, transient_skips
    );
    Ok(())
}

/// Hybrid token resolution. Prefers the client-credentials flow when a user has
/// configured a Spotify Developer app; falls back to an anonymous guest token
/// from `open.spotify.com` when they haven't (or when client-creds fails). The
/// active mode is returned so the loop can calibrate its rate-limit delay.
async fn ensure_token(state: &SharedState, http: &Client) -> Option<(String, SpotifyTokenMode)> {
    let cached: Option<(String, SpotifyTokenMode)> = {
        let s = state.read().await;
        s.spotify_tokens
            .as_ref()
            .filter(|t| !crate::services::spotify::auth::is_expired(t))
            .map(|t| {
                (
                    t.access_token.clone(),
                    crate::services::spotify::auth::token_mode(t),
                )
            })
    };
    if let Some(hit) = cached {
        return Some(hit);
    }

    let creds = state
        .read()
        .await
        .db
        .with_conn(|conn| {
            Ok(crate::services::spotify::auth::load_credentials(conn)
                .ok()
                .flatten())
        })
        .unwrap_or(None);

    match crate::services::spotify::auth::obtain_token(http, creds).await {
        Ok(fresh) => {
            let token = fresh.access_token.clone();
            let mode = crate::services::spotify::auth::token_mode(&fresh);
            let mut s = state.write().await;
            s.spotify_tokens = Some(fresh);
            Some((token, mode))
        }
        Err(e) => {
            warn!("Spotify token resolution failed (both paths): {}", e);
            None
        }
    }
}
