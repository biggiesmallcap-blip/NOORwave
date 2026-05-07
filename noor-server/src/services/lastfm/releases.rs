//! "New Releases" via the Last.fm JSON API only — no HTML scraping.
//!
//! Pipeline (locked by the user's spec):
//!   1. `chart.getTopArtists` (limit ~30)
//!   2. for the top 25 artists, `artist.getTopAlbums` (limit ~5 each)
//!   3. dedupe candidates by mbid OR normalized (artist, album)
//!   4. for the top 30 candidates, `album.getInfo` (by mbid if present)
//!   5. take the high-res image (mega > extralarge), parse `releasedate`
//!   6. keep dated releases <= 90 days old; undated kept but ranked lower
//!   7. sort released_at DESC NULLS LAST, then by parent-artist chart rank ASC
//!   8. return top 12-15
//!
//! Output `ReleaseItem` matches the existing frontend release-card shape, so
//! the frontend type does not change.

use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, NaiveDateTime, Utc};
use futures::stream::{self, StreamExt};
use serde::Serialize;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const ENDPOINT: &str = "https://ws.audioscrobbler.com/2.0/";

const TOP_ARTISTS_LIMIT: u32 = 30;
const ARTISTS_TO_SCAN: usize = 25;
const TOP_ALBUMS_PER_ARTIST: u32 = 5;
const CANDIDATES_FOR_GETINFO: usize = 30;
const CONCURRENT_REQUESTS: usize = 8;
const RECENT_WINDOW_DAYS: i64 = 90;
const RESULT_CAP: usize = 15;
const CACHE_TTL: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Debug, Clone, Serialize)]
pub struct ReleaseItem {
    pub title: String,
    pub link: String,
    pub author: String,
    pub image_url: Option<String>,
    pub source: &'static str,
    /// Best-effort release date in RFC3339. `None` when Last.fm did not
    /// provide a `releasedate` for the album.
    pub published_at: Option<String>,
}

/// Cached entry point. Returns the cached value when fresh; otherwise refreshes
/// in the foreground. Falls back to the previous (stale) cached value if the
/// refresh fails so the home page never goes empty on a transient API blip.
pub async fn fetch_new_releases_cached(
    http: &reqwest::Client,
    api_key: &str,
) -> Result<Vec<ReleaseItem>> {
    {
        let guard = CACHE.lock().unwrap();
        if let Some((items, fetched_at)) = guard.as_ref()
            && fetched_at.elapsed() < CACHE_TTL {
                return Ok(items.clone());
            }
    }
    match fetch_new_releases(http, api_key).await {
        Ok(items) => {
            let mut guard = CACHE.lock().unwrap();
            *guard = Some((items.clone(), Instant::now()));
            Ok(items)
        }
        Err(e) => {
            // On a refresh failure, prefer serving stale data over a hard error.
            let guard = CACHE.lock().unwrap();
            if let Some((items, _)) = guard.as_ref() {
                tracing::warn!("Last.fm releases refresh failed, serving stale cache: {e}");
                Ok(items.clone())
            } else {
                Err(e)
            }
        }
    }
}

static CACHE: LazyLock<Mutex<Option<(Vec<ReleaseItem>, Instant)>>> =
    LazyLock::new(|| Mutex::new(None));

/// Uncached pipeline. Public for tests + cache layer above.
pub async fn fetch_new_releases(http: &reqwest::Client, api_key: &str) -> Result<Vec<ReleaseItem>> {
    let chart_body = call_lastfm(
        http,
        api_key,
        "chart.getTopArtists",
        &[("limit", TOP_ARTISTS_LIMIT.to_string())],
    )
    .await
    .context("chart.getTopArtists")?;
    let top_artists = parse_chart_artists(&chart_body);

    // (artist, rank) for the top N
    let scan_artists: Vec<(String, usize)> = top_artists
        .into_iter()
        .take(ARTISTS_TO_SCAN)
        .enumerate()
        .map(|(i, name)| (name, i))
        .collect();

    // Fetch top albums per artist with bounded concurrency.
    let album_lists: Vec<Vec<AlbumCandidate>> = stream::iter(scan_artists.into_iter())
        .map(|(artist, rank)| {
            let http = http.clone();
            let api_key = api_key.to_string();
            async move {
                match call_lastfm(
                    &http,
                    &api_key,
                    "artist.getTopAlbums",
                    &[
                        ("artist", artist.clone()),
                        ("limit", TOP_ALBUMS_PER_ARTIST.to_string()),
                    ],
                )
                .await
                {
                    Ok(body) => parse_top_albums(&body, &artist, rank),
                    Err(e) => {
                        tracing::debug!("artist.getTopAlbums({artist}): {e}");
                        Vec::new()
                    }
                }
            }
        })
        .buffer_unordered(CONCURRENT_REQUESTS)
        .collect()
        .await;

    let mut candidates: Vec<AlbumCandidate> = album_lists.into_iter().flatten().collect();
    dedupe_candidates(&mut candidates);

    // Sort by parent-artist chart rank ASC so getInfo budget goes to the
    // most-trending entries first.
    candidates.sort_by_key(|c| c.artist_rank);
    candidates.truncate(CANDIDATES_FOR_GETINFO);

    // Enrich each candidate with album.getInfo (image + release date).
    let enriched: Vec<EnrichedAlbum> = stream::iter(candidates.into_iter())
        .map(|cand| {
            let http = http.clone();
            let api_key = api_key.to_string();
            async move {
                let mut params: Vec<(&'static str, String)> = Vec::new();
                if let Some(mbid) = cand.mbid.as_ref() {
                    params.push(("mbid", mbid.clone()));
                } else {
                    params.push(("artist", cand.artist.clone()));
                    params.push(("album", cand.album.clone()));
                }
                match call_lastfm(&http, &api_key, "album.getInfo", &params).await {
                    Ok(body) => Some(parse_album_info(&body, &cand)),
                    Err(e) => {
                        tracing::debug!("album.getInfo({} - {}): {e}", cand.artist, cand.album);
                        // Fall back to the chart-list image we already have.
                        Some(EnrichedAlbum {
                            cand,
                            image_url: None,
                            released_at: None,
                            link: None,
                        })
                    }
                }
            }
        })
        .buffer_unordered(CONCURRENT_REQUESTS)
        .filter_map(|x| async move { x })
        .collect()
        .await;

    let cutoff = (Utc::now() - ChronoDuration::days(RECENT_WINDOW_DAYS)).naive_utc();

    let mut keep: Vec<EnrichedAlbum> = enriched
        .into_iter()
        .filter(|e| match e.released_at {
            Some(dt) => dt >= cutoff,
            // Undated entries are kept (per spec) and ranked below dated.
            None => true,
        })
        .collect();

    // Sort: released_at DESC NULLS LAST, then artist rank ASC.
    keep.sort_by(|a, b| match (a.released_at, b.released_at) {
        (Some(x), Some(y)) => y
            .cmp(&x)
            .then_with(|| a.cand.artist_rank.cmp(&b.cand.artist_rank)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cand.artist_rank.cmp(&b.cand.artist_rank),
    });

    keep.truncate(RESULT_CAP);

    Ok(keep
        .into_iter()
        .map(|e| ReleaseItem {
            title: e.cand.album,
            author: e.cand.artist,
            link: e.link.or_else(|| e.cand.url.clone()).unwrap_or_default(),
            image_url: e.image_url.or_else(|| e.cand.image_url.clone()),
            source: "lastfm_api",
            published_at: e
                .released_at
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
        })
        .collect())
}

// ─── Internal ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct AlbumCandidate {
    artist: String,
    album: String,
    mbid: Option<String>,
    image_url: Option<String>,
    url: Option<String>,
    artist_rank: usize,
}

#[derive(Debug)]
struct EnrichedAlbum {
    cand: AlbumCandidate,
    image_url: Option<String>,
    released_at: Option<NaiveDateTime>,
    link: Option<String>,
}

async fn call_lastfm(
    http: &reqwest::Client,
    api_key: &str,
    method: &str,
    extra: &[(&'static str, String)],
) -> Result<serde_json::Value> {
    let mut req =
        http.get(ENDPOINT)
            .query(&[("method", method), ("api_key", api_key), ("format", "json")]);
    for (k, v) in extra {
        req = req.query(&[(*k, v.as_str())]);
    }
    let resp = req.send().await.context("send")?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.context("decode json")?;
    if let Some(err) = body.get("error").and_then(serde_json::Value::as_i64) {
        anyhow::bail!(
            "Last.fm error {}: {} (HTTP {})",
            err,
            body.get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(""),
            status
        );
    }
    if !status.is_success() {
        anyhow::bail!("Last.fm HTTP {}", status);
    }
    Ok(body)
}

fn parse_chart_artists(body: &serde_json::Value) -> Vec<String> {
    body.get("artists")
        .and_then(|v| v.get("artist"))
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    a.get("name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_top_albums(
    body: &serde_json::Value,
    artist: &str,
    artist_rank: usize,
) -> Vec<AlbumCandidate> {
    let Some(arr) = body
        .get("topalbums")
        .and_then(|v| v.get("album"))
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|a| {
            let album = a
                .get("name")
                .and_then(serde_json::Value::as_str)?
                .to_string();
            // Last.fm sentinel for unknown album.
            if album.eq_ignore_ascii_case("(null)") || album.is_empty() {
                return None;
            }
            let mbid = a
                .get("mbid")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let url = a
                .get("url")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let image_url = pick_image(a.get("image"), &["mega", "extralarge", "large"]);
            Some(AlbumCandidate {
                artist: artist.to_string(),
                album,
                mbid,
                image_url,
                url,
                artist_rank,
            })
        })
        .collect()
}

fn parse_album_info(body: &serde_json::Value, cand: &AlbumCandidate) -> EnrichedAlbum {
    let album = body.get("album");
    let image_url = album.and_then(|a| a.get("image"));
    let image_url = pick_image(image_url, &["mega", "extralarge", "large"]);
    let link = album
        .and_then(|a| a.get("url"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let released_at = album
        .and_then(|a| a.get("releasedate"))
        .and_then(serde_json::Value::as_str)
        .and_then(parse_release_date)
        .or_else(|| {
            album
                .and_then(|a| a.get("wiki"))
                .and_then(|w| w.get("published"))
                .and_then(serde_json::Value::as_str)
                .and_then(parse_release_date)
        });
    EnrichedAlbum {
        cand: cand.clone(),
        image_url,
        released_at,
        link,
    }
}

/// Last.fm releasedate strings look like `"    6 Sep 2024, 00:00"` (leading
/// whitespace, day-name optional). We accept both that format and the wiki's
/// `"08 Mar 2008, 16:01"`.
fn parse_release_date(raw: &str) -> Option<NaiveDateTime> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    NaiveDateTime::parse_from_str(trimmed, "%e %b %Y, %H:%M")
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(trimmed, "%d %b %Y, %H:%M").ok())
}

fn pick_image(images: Option<&serde_json::Value>, sizes: &[&str]) -> Option<String> {
    let arr = images?.as_array()?;
    for size in sizes {
        for img in arr {
            let s = img.get("size").and_then(serde_json::Value::as_str);
            let url = img.get("#text").and_then(serde_json::Value::as_str);
            if s == Some(*size)
                && let Some(u) = url
                    && !u.is_empty() {
                        return Some(u.to_string());
                    }
        }
    }
    None
}

fn dedupe_candidates(candidates: &mut Vec<AlbumCandidate>) {
    use std::collections::HashSet;
    let mut seen_mbid: HashSet<String> = HashSet::new();
    let mut seen_pair: HashSet<(String, String)> = HashSet::new();
    candidates.retain(|c| {
        if let Some(mbid) = c.mbid.as_ref()
            && !seen_mbid.insert(mbid.clone()) {
                return false;
            }
        let key = (c.artist.to_lowercase(), c.album.to_lowercase());
        if !seen_pair.insert(key) {
            return false;
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_chart_top_artists() {
        let body = json!({
            "artists": {
                "artist": [
                    { "name": "Artist A" },
                    { "name": "Artist B" },
                    { "name": "" }
                ]
            }
        });
        let names = parse_chart_artists(&body);
        // Empty names are still surfaced — filtering happens at top_albums where
        // an empty artist would just yield no albums. We keep this lenient so
        // a single bad row in Last.fm's response doesn't drop neighbors.
        assert!(names.contains(&"Artist A".to_string()));
        assert!(names.contains(&"Artist B".to_string()));
    }

    #[test]
    fn parses_top_albums_skips_null_sentinel() {
        let body = json!({
            "topalbums": {
                "album": [
                    {
                        "name": "Real Album",
                        "mbid": "abc-123",
                        "url": "https://www.last.fm/music/Foo/Real+Album",
                        "image": [
                            { "#text": "https://img.example/sm.png", "size": "small" },
                            { "#text": "https://img.example/xl.png", "size": "extralarge" }
                        ]
                    },
                    { "name": "(null)" },
                    { "name": "" }
                ]
            }
        });
        let cands = parse_top_albums(&body, "Foo", 7);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].album, "Real Album");
        assert_eq!(cands[0].artist, "Foo");
        assert_eq!(cands[0].artist_rank, 7);
        assert_eq!(cands[0].mbid.as_deref(), Some("abc-123"));
        assert_eq!(
            cands[0].image_url.as_deref(),
            Some("https://img.example/xl.png")
        );
    }

    #[test]
    fn pick_image_prefers_mega() {
        let v = json!([
            { "size": "small",      "#text": "small.png" },
            { "size": "extralarge", "#text": "xl.png" },
            { "size": "mega",       "#text": "mega.png" }
        ]);
        assert_eq!(
            pick_image(Some(&v), &["mega", "extralarge"]).as_deref(),
            Some("mega.png")
        );
    }

    #[test]
    fn pick_image_falls_back_to_smaller_when_mega_missing() {
        let v = json!([
            { "size": "small",      "#text": "small.png" },
            { "size": "extralarge", "#text": "xl.png" }
        ]);
        assert_eq!(
            pick_image(Some(&v), &["mega", "extralarge"]).as_deref(),
            Some("xl.png")
        );
    }

    #[test]
    fn parse_release_date_handles_padded_day() {
        // Last.fm's "DD Mon YYYY, HH:MM" with leading-space single-digit day.
        let dt = parse_release_date(" 6 Sep 2024, 00:00").unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2024-09-06");

        let dt = parse_release_date("23 Mar 2025, 12:30").unwrap();
        assert_eq!(dt.format("%Y-%m-%d").to_string(), "2025-03-23");

        assert!(parse_release_date("").is_none());
        assert!(parse_release_date("garbage").is_none());
    }

    #[test]
    fn parse_album_info_extracts_image_link_and_date() {
        let body = json!({
            "album": {
                "name": "Real Album",
                "url": "https://www.last.fm/music/Foo/Real+Album",
                "image": [
                    { "size": "extralarge", "#text": "https://img.example/xl.png" },
                    { "size": "mega",       "#text": "https://img.example/mega.png" }
                ],
                "releasedate": " 6 Sep 2024, 00:00"
            }
        });
        let cand = AlbumCandidate {
            artist: "Foo".into(),
            album: "Real Album".into(),
            mbid: None,
            image_url: None,
            url: None,
            artist_rank: 0,
        };
        let e = parse_album_info(&body, &cand);
        assert_eq!(e.image_url.as_deref(), Some("https://img.example/mega.png"));
        assert_eq!(
            e.link.as_deref(),
            Some("https://www.last.fm/music/Foo/Real+Album")
        );
        assert!(e.released_at.is_some());
    }

    #[test]
    fn dedupe_by_mbid_and_normalized_pair() {
        let mut cs = vec![
            AlbumCandidate {
                artist: "Foo".into(),
                album: "Bar".into(),
                mbid: Some("aaa".into()),
                image_url: None,
                url: None,
                artist_rank: 0,
            },
            AlbumCandidate {
                artist: "Foo".into(),
                album: "Bar".into(),
                mbid: Some("aaa".into()),
                image_url: None,
                url: None,
                artist_rank: 1,
            },
            AlbumCandidate {
                artist: "FOO".into(),
                album: "bar".into(),
                mbid: None,
                image_url: None,
                url: None,
                artist_rank: 2,
            },
            AlbumCandidate {
                artist: "Other".into(),
                album: "Different".into(),
                mbid: None,
                image_url: None,
                url: None,
                artist_rank: 3,
            },
        ];
        dedupe_candidates(&mut cs);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].artist, "Foo");
        assert_eq!(cs[1].artist, "Other");
    }
}
