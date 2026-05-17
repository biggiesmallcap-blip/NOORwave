//! ISRC -> Spotify track + artist-id resolution heuristics.
//!
//! Resolver lives upstream of the cache: it picks *which* Spotify ID a given
//! Tidal-side ISRC or artist name should map to. The cache later just stores
//! the chosen ID.
//!
//! Strategy:
//!   - ISRC search: `isrc:<code>`, fall back to `<artist> <title>`. Score
//!     candidates by exact title + primary-artist match, then by playcount,
//!     then by popularity.
//!   - Artist resolution: walk up to 15 sample ISRCs, getTrack each, take
//!     the primary-artist URI whose name matches the Tidal artist name
//!     (case-insensitive). Fall back to searchModalResults if no ISRC has
//!     resolved.

use anyhow::Result;
use serde_json::Value;
use tracing::warn;

use super::client::SpotifyPublicClient;

#[derive(Debug, Clone)]
pub struct ResolvedTrack {
    pub spotify_track_id: String,
    pub playcount: Option<i64>,
}

fn norm(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_ascii_punctuation())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_track_uri(uri: &str) -> Option<String> {
    uri.strip_prefix("spotify:track:").map(|s| s.to_string())
}

fn parse_artist_uri(uri: &str) -> Option<String> {
    uri.strip_prefix("spotify:artist:").map(|s| s.to_string())
}

/// Pull `tracks.items[]` candidate URIs out of a searchModalResults payload.
fn search_track_uris(body: &Value) -> Vec<String> {
    let Some(items) = body
        .pointer("/data/searchV2/tracksV2/items")
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            item.pointer("/item/data/uri")
                .or_else(|| item.pointer("/data/uri"))
                .and_then(|v| v.as_str())
                .and_then(parse_track_uri)
        })
        .collect()
}

/// Resolve one ISRC to a Spotify track id + playcount. Returns `None` if
/// nothing matched within the heuristic threshold.
pub async fn resolve_track_for_isrc(
    client: &SpotifyPublicClient,
    isrc: &str,
    title: &str,
    artist_name: &str,
) -> Result<Option<ResolvedTrack>> {
    let norm_title = norm(title);
    let norm_artist = norm(artist_name);

    let mut candidate_ids = Vec::new();
    let primary_query = format!("isrc:{isrc}");
    match client.search(&primary_query).await {
        Ok(body) => {
            for tid in search_track_uris(&body) {
                if !candidate_ids.contains(&tid) {
                    candidate_ids.push(tid);
                }
            }
        }
        Err(e) => warn!("spotify_public: isrc search failed for {isrc}: {e:#}"),
    }
    if candidate_ids.is_empty() {
        let q = format!("{artist_name} {title}");
        match client.search(&q).await {
            Ok(body) => {
                for tid in search_track_uris(&body) {
                    if !candidate_ids.contains(&tid) {
                        candidate_ids.push(tid);
                    }
                }
            }
            Err(e) => warn!("spotify_public: fallback search failed for {isrc}: {e:#}"),
        }
    }

    let mut best: Option<ResolvedTrack> = None;
    for tid in candidate_ids.into_iter().take(5) {
        let body = match client.get_track(&tid).await {
            Ok(b) => b,
            Err(e) => {
                warn!("spotify_public: getTrack({tid}) failed: {e:#}");
                continue;
            }
        };

        let track = body.pointer("/data/trackUnion").unwrap_or(&body);
        let returned_title = track
            .pointer("/name")
            .and_then(|v| v.as_str())
            .map(norm)
            .unwrap_or_default();
        let primary_artist_name = track
            .pointer("/artistsWithRoles/items/0/artist/profile/name")
            .or_else(|| track.pointer("/artists/items/0/profile/name"))
            .and_then(|v| v.as_str())
            .map(norm)
            .unwrap_or_default();
        let playcount = track
            .pointer("/playcount")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| track.pointer("/playcount").and_then(|v| v.as_i64()));

        let title_match = returned_title == norm_title;
        let artist_match = primary_artist_name == norm_artist;
        if !title_match || !artist_match {
            // Skip mismatches outright; ISRC search should normally only
            // return exact matches, but reissues / live versions can leak in.
            continue;
        }
        let candidate = ResolvedTrack {
            spotify_track_id: tid,
            playcount,
        };
        best = Some(match (best.take(), candidate) {
            (None, c) => c,
            (Some(prev), c) => {
                if c.playcount.unwrap_or(0) > prev.playcount.unwrap_or(0) {
                    c
                } else {
                    prev
                }
            }
        });
    }

    Ok(best)
}

/// Walk sample ISRCs to pivot to the artist's Spotify ID. Returns `None` if
/// no track resolution returns a primary-artist name matching `tidal_artist_name`.
pub async fn resolve_artist_id(
    client: &SpotifyPublicClient,
    tidal_artist_name: &str,
    sample_isrcs_titles: &[(String, String)],
) -> Result<Option<String>> {
    let norm_artist = norm(tidal_artist_name);

    for (isrc, title) in sample_isrcs_titles.iter().take(15) {
        let q = format!("isrc:{isrc}");
        let body = match client.search(&q).await {
            Ok(b) => b,
            Err(e) => {
                warn!("spotify_public: isrc search for artist resolution failed: {e:#}");
                continue;
            }
        };
        let Some(tid) = search_track_uris(&body).into_iter().next() else {
            continue;
        };
        let track_body = match client.get_track(&tid).await {
            Ok(b) => b,
            Err(e) => {
                warn!("spotify_public: getTrack while resolving artist failed: {e:#}");
                continue;
            }
        };
        let track = track_body.pointer("/data/trackUnion").unwrap_or(&track_body);

        let returned_title = track
            .pointer("/name")
            .and_then(|v| v.as_str())
            .map(norm)
            .unwrap_or_default();
        if returned_title != norm(title) {
            continue;
        }

        let Some(items) = track
            .pointer("/artistsWithRoles/items")
            .or_else(|| track.pointer("/artists/items"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        for art in items {
            let name = art
                .pointer("/artist/profile/name")
                .or_else(|| art.pointer("/profile/name"))
                .and_then(|v| v.as_str())
                .map(norm)
                .unwrap_or_default();
            if name != norm_artist {
                continue;
            }
            let uri = art
                .pointer("/artist/uri")
                .or_else(|| art.pointer("/uri"))
                .and_then(|v| v.as_str());
            if let Some(spid) = uri.and_then(parse_artist_uri) {
                return Ok(Some(spid));
            }
        }
    }

    // Fallback: direct artist search.
    if let Ok(body) = client.search(tidal_artist_name).await {
        let Some(items) = body
            .pointer("/data/searchV2/artists/items")
            .and_then(|v| v.as_array())
        else {
            return Ok(None);
        };
        for item in items.iter().take(5) {
            let name = item
                .pointer("/data/profile/name")
                .and_then(|v| v.as_str())
                .map(norm)
                .unwrap_or_default();
            if name != norm_artist {
                continue;
            }
            let uri = item
                .pointer("/data/uri")
                .and_then(|v| v.as_str())
                .and_then(parse_artist_uri);
            if let Some(spid) = uri {
                return Ok(Some(spid));
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_strips_punctuation_and_collapses_whitespace() {
        assert_eq!(norm("  Daft   Punk!! "), "daft punk");
        assert_eq!(norm("Born  in the U.S.A."), "born in the usa");
    }

    #[test]
    fn parse_track_uri_extracts_id() {
        assert_eq!(
            parse_track_uri("spotify:track:abc123").as_deref(),
            Some("abc123")
        );
        assert_eq!(parse_track_uri("spotify:artist:abc"), None);
    }
}
