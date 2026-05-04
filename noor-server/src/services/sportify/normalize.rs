//! Normalize Sportify DTOs into the discovery shape consumed by /discover.
//!
//! Output is camelCase JSON to match the design spec the user pinned. Each
//! playable row carries a `tidal: { status, id, confidence, ... }` block;
//! cache-resolved status is filled in by [`enrich_tracks_with_tidal_cache`],
//! everything else lands as `pending`. Phase 4 will turn pending into
//! resolved/unresolved via the eager-first-N + lazy-rest pipeline.

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use super::cache::{self as sp_cache, SportifyCacheConfig};
use super::models::{
    SportifyAlbum, SportifyArtist, SportifyArtistRef, SportifyImage, SportifyPlaylist,
    SportifySearchResults, SportifyTrack,
};
// Owner accessor is on the model enum; pull it via SportifyPlaylistOwner.
use super::resolver::{self, ResolutionStatus};

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ArtistRef {
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TidalState {
    /// `pending` | `resolved` | `low_confidence` | `unresolved` | `error`
    pub status: String,
    pub id: Option<i64>,
    pub confidence: f64,
    pub match_reason: Option<String>,
    pub from_cache: bool,
}

impl TidalState {
    pub fn pending() -> Self {
        Self {
            status: "pending".to_string(),
            id: None,
            confidence: 0.0,
            match_reason: None,
            from_cache: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryMeta {
    pub score: f64,
    pub contexts: Vec<String>,
    pub seeds: Vec<String>,
    pub source_endpoint: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryTrack {
    pub source: &'static str,
    pub spotify_id: Option<String>,
    pub r#type: &'static str,
    pub title: Option<String>,
    pub primary_artist: Option<String>,
    pub artists: Vec<ArtistRef>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub thumbnail: Option<String>,
    pub duration_ms: Option<i64>,
    pub release_date: Option<String>,
    pub explicit: Option<bool>,
    pub track_number: Option<i32>,
    pub disc_number: Option<i32>,
    pub spotify_url: Option<String>,
    pub preview_url: Option<String>,
    pub playcount: Option<i64>,
    pub popularity: Option<i32>,
    pub isrc: Option<String>,
    pub tidal: TidalState,
    pub discovery: DiscoveryMeta,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryAlbum {
    pub source: &'static str,
    pub spotify_id: Option<String>,
    pub r#type: &'static str,
    pub title: Option<String>,
    pub primary_artist: Option<String>,
    pub artists: Vec<ArtistRef>,
    pub thumbnail: Option<String>,
    pub release_date: Option<String>,
    pub total_tracks: Option<i32>,
    pub album_type: Option<String>,
    pub label: Option<String>,
    pub genres: Vec<String>,
    pub spotify_url: Option<String>,
    pub tracks: Vec<DiscoveryTrack>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryArtist {
    pub source: &'static str,
    pub spotify_id: Option<String>,
    pub r#type: &'static str,
    pub name: Option<String>,
    pub thumbnail: Option<String>,
    pub genres: Vec<String>,
    pub popularity: Option<i32>,
    pub monthly_listeners: Option<i64>,
    pub followers: Option<i64>,
    pub world_rank: Option<i64>,
    pub biography: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveryPlaylist {
    pub source: &'static str,
    pub spotify_id: Option<String>,
    pub r#type: &'static str,
    pub title: Option<String>,
    pub description: Option<String>,
    pub thumbnail: Option<String>,
    pub owner: Option<String>,
    pub followers: Option<i64>,
    pub total_tracks: Option<i32>,
    pub snapshot_id: Option<String>,
    pub tracks: Vec<DiscoveryTrack>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverySearchResults {
    pub tracks: Vec<DiscoveryTrack>,
    pub albums: Vec<DiscoveryAlbum>,
    pub artists: Vec<DiscoveryArtist>,
    pub playlists: Vec<DiscoveryPlaylist>,
}

// ─── Conversions ─────────────────────────────────────────────

fn pick_image(images: &[SportifyImage]) -> Option<String> {
    images
        .iter()
        .max_by_key(|i| i.width.unwrap_or(0))
        .and_then(|i| i.url.clone())
}

fn artist_refs(artists: &[SportifyArtistRef]) -> Vec<ArtistRef> {
    artists
        .iter()
        .map(|a| ArtistRef {
            id: a.id.clone(),
            name: a.name.clone(),
        })
        .collect()
}

fn primary_artist_name(artists: &[SportifyArtistRef]) -> Option<String> {
    artists.first().and_then(|a| a.name.clone())
}

fn spotify_url_from_map(map: &std::collections::HashMap<String, String>, fallback_id: Option<&str>, kind: &str) -> Option<String> {
    if let Some(url) = map.get("spotify").cloned() {
        return Some(url);
    }
    fallback_id.map(|id| format!("https://open.spotify.com/{}/{}", kind, id))
}

pub fn track_from_sportify(t: &SportifyTrack, source_endpoint: &str) -> DiscoveryTrack {
    let isrc = t.external_ids.as_ref().and_then(|e| e.isrc.clone());
    // Spotify URL: prefer the flat `url` Sportify ships on the track body,
    // then the typed `external_urls.spotify`, then build one from the id.
    let spotify_url = t
        .url
        .clone()
        .or_else(|| spotify_url_from_map(&t.external_urls, t.id.as_deref(), "track"));

    DiscoveryTrack {
        source: "spotify",
        spotify_id: t.id.clone(),
        r#type: "track",
        title: t.name.clone(),
        primary_artist: t.primary_artist().map(str::to_string),
        // Synthesize an artists array from the flat `artist` string when the
        // structured `artists` field is absent (playlist/search shape).
        artists: if !t.artists.is_empty() {
            artist_refs(&t.artists)
        } else if let Some(name) = t.artist.clone() {
            vec![ArtistRef { id: None, name: Some(name) }]
        } else {
            Vec::new()
        },
        album: t.album.as_ref().and_then(|a| a.name.clone()),
        album_id: t.album.as_ref().and_then(|a| a.id.clone()),
        thumbnail: t.best_thumbnail(),
        duration_ms: t.duration_ms,
        release_date: t
            .album
            .as_ref()
            .and_then(|a| a.release_date.clone())
            .or_else(|| {
                // Track-detail responses sometimes ship release_date on the
                // track itself rather than the album block.
                t.extra
                    .get("release_date")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            }),
        explicit: t.explicit,
        track_number: t.track_number,
        disc_number: t.disc_number,
        spotify_url,
        preview_url: t.preview_url.clone(),
        playcount: t.playcount,
        popularity: t.popularity,
        isrc,
        tidal: TidalState::pending(),
        discovery: DiscoveryMeta {
            source_endpoint: source_endpoint.to_string(),
            ..DiscoveryMeta::default()
        },
    }
}

pub fn album_from_sportify(a: &SportifyAlbum, source_endpoint: &str) -> DiscoveryAlbum {
    let thumb = pick_image(&a.images);
    let tracks = a
        .tracks
        .iter()
        .map(|t| {
            let mut row = track_from_sportify(t, source_endpoint);
            // Tracks fetched as part of an album payload often omit the
            // album block — backfill it from the parent so downstream UI has
            // artwork + album name without an extra fetch.
            if row.album.is_none() {
                row.album = a.name.clone();
            }
            if row.album_id.is_none() {
                row.album_id = a.id.clone();
            }
            if row.thumbnail.is_none() {
                row.thumbnail = thumb.clone();
            }
            if row.release_date.is_none() {
                row.release_date = a.release_date.clone();
            }
            row
        })
        .collect();
    DiscoveryAlbum {
        source: "spotify",
        spotify_id: a.id.clone(),
        r#type: "album",
        title: a.name.clone(),
        primary_artist: primary_artist_name(&a.artists),
        artists: artist_refs(&a.artists),
        thumbnail: thumb,
        release_date: a.release_date.clone(),
        total_tracks: a.total_tracks,
        album_type: a.album_type.clone(),
        label: a.label.clone(),
        genres: a.genres.clone(),
        spotify_url: a
            .id
            .as_deref()
            .map(|id| format!("https://open.spotify.com/album/{}", id)),
        tracks,
    }
}

pub fn artist_from_sportify(a: &SportifyArtist) -> DiscoveryArtist {
    DiscoveryArtist {
        source: "spotify",
        spotify_id: a.id.clone(),
        r#type: "artist",
        name: a.name.clone(),
        thumbnail: pick_image(&a.images),
        genres: a.genres.clone(),
        popularity: a.popularity,
        monthly_listeners: a.monthly_listeners,
        followers: a.followers,
        world_rank: a.world_rank,
        biography: a.biography.clone(),
    }
}

pub fn playlist_from_sportify(p: &SportifyPlaylist, source_endpoint: &str) -> DiscoveryPlaylist {
    DiscoveryPlaylist {
        source: "spotify",
        spotify_id: p.spotify_id(),
        r#type: "playlist",
        title: p.title(),
        description: p.description.clone(),
        thumbnail: p.best_thumbnail(),
        owner: p.owner.as_ref().and_then(|o| o.display_name().map(str::to_string)),
        followers: p.follower_count(),
        total_tracks: p.total_track_count(),
        snapshot_id: p.snapshot_id.clone(),
        tracks: p
            .tracks
            .iter()
            .map(|t| track_from_sportify(t, source_endpoint))
            .collect(),
    }
}

pub fn search_from_sportify(
    s: &SportifySearchResults,
    source_endpoint: &str,
) -> DiscoverySearchResults {
    DiscoverySearchResults {
        tracks: s
            .tracks
            .iter()
            .map(|t| track_from_sportify(t, source_endpoint))
            .collect(),
        albums: s
            .albums
            .iter()
            .map(|a| album_from_sportify(a, source_endpoint))
            .collect(),
        artists: s.artists.iter().map(artist_from_sportify).collect(),
        playlists: s
            .playlists
            .iter()
            .map(|p| playlist_from_sportify(p, source_endpoint))
            .collect(),
    }
}

// ─── Tidal cache enrichment ──────────────────────────────────

/// Fill in each track's `tidal:` block from the resolution cache. Tracks
/// without a cache hit keep their `pending` state.
pub fn enrich_tracks_with_tidal_cache(
    conn: &Connection,
    cfg: &SportifyCacheConfig,
    tracks: &mut [DiscoveryTrack],
) -> Result<()> {
    for track in tracks.iter_mut() {
        let Some(spotify_id) = track.spotify_id.as_deref() else {
            continue;
        };
        if let Some(hit) = sp_cache::get_tidal_resolution(conn, cfg, spotify_id)? {
            track.tidal = TidalState {
                status: resolver::classify(hit.confidence).as_str().to_string(),
                id: Some(hit.tidal_track_id),
                confidence: hit.confidence,
                match_reason: hit.match_reason,
                from_cache: true,
            };
            continue;
        }
        if let Some(record) = sp_cache::get_unresolved(conn, spotify_id)? {
            if sp_cache::unresolved_is_cold(&record, cfg) {
                track.tidal = TidalState {
                    status: ResolutionStatus::Unresolved.as_str().to_string(),
                    id: None,
                    confidence: 0.0,
                    match_reason: record.reason,
                    from_cache: true,
                };
            }
        }
    }
    Ok(())
}
