//! Sportify response DTOs.
//!
//! Sportify's underlying data sources (Spotify embed scraping + Spotify
//! partner GraphQL + MusicBrainz fallback) mean fields can be sparse. We
//! deserialize permissively: every nested struct uses `#[serde(default)]`
//! and unknown fields are kept in `extra` for forward compatibility.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SportifyImage {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub width: Option<i32>,
    #[serde(default)]
    pub height: Option<i32>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SportifyArtistRef {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, rename = "uri")]
    pub uri: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SportifyAlbumRef {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub images: Vec<SportifyImage>,
    #[serde(default, rename = "release_date")]
    pub release_date: Option<String>,
    #[serde(default, rename = "total_tracks")]
    pub total_tracks: Option<i32>,
}

/// External IDs block (Spotify exposes `isrc`, `ean`, `upc`).
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SportifyExternalIds {
    #[serde(default)]
    pub isrc: Option<String>,
    #[serde(default)]
    pub ean: Option<String>,
    #[serde(default)]
    pub upc: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SportifyTrack {
    #[serde(default)]
    pub id: Option<String>,
    /// Sportify uses `title` for the track name; some shapes use `name`.
    /// Alias picks up either form into one field.
    #[serde(default, alias = "title")]
    pub name: Option<String>,
    /// Some endpoints (track detail) ship a structured `artists` array.
    #[serde(default)]
    pub artists: Vec<SportifyArtistRef>,
    /// Other endpoints (playlist body, search results) ship a flat `artist`
    /// string only. Consumers go through [`SportifyTrack::primary_artist`].
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub album: Option<SportifyAlbumRef>,
    /// Top-level thumbnail URL (track detail + playlist track shape). Album
    /// artwork lives here when there's no nested `album.images`.
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default, rename = "duration_ms")]
    pub duration_ms: Option<i64>,
    #[serde(default)]
    pub explicit: Option<bool>,
    #[serde(default, rename = "track_number")]
    pub track_number: Option<i32>,
    #[serde(default, rename = "disc_number")]
    pub disc_number: Option<i32>,
    #[serde(default, rename = "preview_url")]
    pub preview_url: Option<String>,
    #[serde(default)]
    pub popularity: Option<i32>,
    #[serde(default, rename = "playcount")]
    pub playcount: Option<i64>,
    #[serde(default, rename = "external_ids")]
    pub external_ids: Option<SportifyExternalIds>,
    #[serde(default, rename = "external_urls")]
    pub external_urls: HashMap<String, String>,
    /// Spotify-facing URL on the track body (search/playlist shape).
    #[serde(default)]
    pub url: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl SportifyTrack {
    /// Primary artist name. Prefers the structured `artists` array; falls
    /// back to the flat `artist` string.
    pub fn primary_artist(&self) -> Option<&str> {
        self.artists
            .first()
            .and_then(|a| a.name.as_deref())
            .or(self.artist.as_deref())
    }

    /// Best-available artwork URL for this track. Tries top-level
    /// `thumbnail`, then nested album image, then nothing.
    pub fn best_thumbnail(&self) -> Option<String> {
        if let Some(t) = self.thumbnail.as_deref() {
            return Some(t.to_string());
        }
        self.album
            .as_ref()
            .and_then(|a| a.images.iter().max_by_key(|i| i.width.unwrap_or(0)))
            .and_then(|i| i.url.clone())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SportifyAlbum {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub artists: Vec<SportifyArtistRef>,
    #[serde(default)]
    pub images: Vec<SportifyImage>,
    #[serde(default, rename = "release_date")]
    pub release_date: Option<String>,
    #[serde(default, rename = "release_date_precision")]
    pub release_date_precision: Option<String>,
    #[serde(default, rename = "total_tracks")]
    pub total_tracks: Option<i32>,
    #[serde(default, rename = "album_type")]
    pub album_type: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub tracks: Vec<SportifyTrack>,
    #[serde(default, rename = "external_ids")]
    pub external_ids: Option<SportifyExternalIds>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SportifyArtist {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub images: Vec<SportifyImage>,
    #[serde(default)]
    pub popularity: Option<i32>,
    #[serde(default, rename = "monthly_listeners")]
    pub monthly_listeners: Option<i64>,
    #[serde(default)]
    pub followers: Option<i64>,
    #[serde(default, rename = "world_rank")]
    pub world_rank: Option<i64>,
    #[serde(default)]
    pub biography: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Owner can come back as either a flat string ("Lofi Girl") or a nested
/// object ({ id, display_name }). Untagged so serde tries each in turn —
/// upstream shape changes don't break the whole deserialization.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SportifyPlaylistOwner {
    Name(String),
    Object {
        #[serde(default)]
        id: Option<String>,
        #[serde(default, alias = "display_name")]
        name: Option<String>,
    },
}

impl SportifyPlaylistOwner {
    pub fn display_name(&self) -> Option<&str> {
        match self {
            Self::Name(s) => Some(s.as_str()),
            Self::Object { name, .. } => name.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SportifyPlaylist {
    #[serde(default, alias = "spotify_id", alias = "spotifyId")]
    pub id: Option<String>,
    #[serde(default, alias = "title")]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Flat thumbnail URL the search/playlist endpoints actually return.
    #[serde(
        default,
        alias = "image_url",
        alias = "imageUrl",
        alias = "artwork_url",
        alias = "artworkUrl",
        alias = "cover"
    )]
    pub thumbnail: Option<String>,
    /// Some endpoints return an `images` array instead. Pick whichever is
    /// present in [`crate::services::sportify::normalize`].
    #[serde(default)]
    pub images: Vec<SportifyImage>,
    #[serde(default)]
    pub owner: Option<SportifyPlaylistOwner>,
    #[serde(default, alias = "follower_count", alias = "followerCount")]
    pub followers: Option<i64>,
    #[serde(default, rename = "snapshot_id", alias = "snapshotId")]
    pub snapshot_id: Option<String>,
    #[serde(
        default,
        rename = "total_tracks",
        alias = "totalTracks",
        alias = "track_count",
        alias = "trackCount"
    )]
    pub total_tracks: Option<i32>,
    /// Spotify-facing URL on the playlist body (search shape).
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub tracks: Vec<SportifyTrack>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl SportifyPlaylist {
    pub fn spotify_id(&self) -> Option<String> {
        self.id
            .clone()
            .or_else(|| extra_string(&self.extra, "spotify_id"))
            .or_else(|| extra_string(&self.extra, "spotifyId"))
            .or_else(|| extra_string(&self.extra, "id"))
    }

    pub fn title(&self) -> Option<String> {
        self.name
            .clone()
            .or_else(|| extra_string(&self.extra, "title"))
            .or_else(|| extra_string(&self.extra, "name"))
    }

    pub fn best_thumbnail(&self) -> Option<String> {
        self.images
            .iter()
            .max_by_key(|i| i.width.unwrap_or(0))
            .and_then(|i| i.url.clone())
            .or_else(|| self.thumbnail.clone())
            .or_else(|| extra_string(&self.extra, "image_url"))
            .or_else(|| extra_string(&self.extra, "imageUrl"))
            .or_else(|| extra_string(&self.extra, "artwork_url"))
            .or_else(|| extra_string(&self.extra, "artworkUrl"))
            .or_else(|| extra_string(&self.extra, "cover"))
    }

    pub fn follower_count(&self) -> Option<i64> {
        self.followers
            .or_else(|| extra_i64(&self.extra, "follower_count"))
            .or_else(|| extra_i64(&self.extra, "followerCount"))
            .or_else(|| extra_i64(&self.extra, "followers"))
    }

    pub fn total_track_count(&self) -> Option<i32> {
        self.total_tracks
            .or_else(|| extra_i64(&self.extra, "total_tracks").and_then(i64_to_i32))
            .or_else(|| extra_i64(&self.extra, "totalTracks").and_then(i64_to_i32))
            .or_else(|| extra_i64(&self.extra, "track_count").and_then(i64_to_i32))
            .or_else(|| extra_i64(&self.extra, "trackCount").and_then(i64_to_i32))
    }
}

fn extra_string(extra: &HashMap<String, serde_json::Value>, key: &str) -> Option<String> {
    extra.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn extra_i64(extra: &HashMap<String, serde_json::Value>, key: &str) -> Option<i64> {
    extra.get(key).and_then(|v| v.as_i64())
}

fn i64_to_i32(value: i64) -> Option<i32> {
    i32::try_from(value).ok()
}

/// Search response. Fields are individually optional because Sportify only
/// returns the bucket matching the requested `type`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct SportifySearchResults {
    #[serde(default)]
    pub tracks: Vec<SportifyTrack>,
    #[serde(default)]
    pub albums: Vec<SportifyAlbum>,
    #[serde(default)]
    pub artists: Vec<SportifyArtist>,
    #[serde(default)]
    pub playlists: Vec<SportifyPlaylist>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: playlist-body track shape uses `title` + flat `artist`
    /// + top-level `thumbnail`. The first ship missed this and every track
    /// deserialized into all-None defaults.
    #[test]
    fn deserializes_playlist_track_shape() {
        let json = r#"{
            "id": "6bp0DUXnsAdLbueBfFTPFn",
            "title": "The Descent",
            "artist": "Blue Wednesday",
            "thumbnail": "https://example.com/thumb.jpg",
            "duration": "3:33",
            "duration_ms": 213038,
            "explicit": false,
            "preview_url": "https://example.com/preview.mp3",
            "url": "https://open.spotify.com/track/6bp0DUXnsAdLbueBfFTPFn"
        }"#;
        let track: SportifyTrack = serde_json::from_str(json).expect("playlist track parse");
        assert_eq!(track.id.as_deref(), Some("6bp0DUXnsAdLbueBfFTPFn"));
        assert_eq!(track.name.as_deref(), Some("The Descent"));
        assert_eq!(track.primary_artist(), Some("Blue Wednesday"));
        assert_eq!(track.duration_ms, Some(213038));
        assert_eq!(
            track.best_thumbnail().as_deref(),
            Some("https://example.com/thumb.jpg"),
        );
    }

    /// Track-detail shape ships both `artists` array AND flat `artist`.
    /// `primary_artist()` should prefer the structured array.
    #[test]
    fn primary_artist_prefers_structured_array() {
        let json = r#"{
            "id": "x",
            "title": "T",
            "artist": "Flat Name",
            "artists": [{ "id": "a1", "name": "Structured Name" }]
        }"#;
        let track: SportifyTrack = serde_json::from_str(json).expect("parse");
        assert_eq!(track.primary_artist(), Some("Structured Name"));
    }

    /// Playlist owner can be a plain string ("Lofi Girl") or a nested
    /// `{ id, display_name }`. Both must round-trip through the enum.
    #[test]
    fn playlist_owner_accepts_either_shape() {
        let flat: SportifyPlaylist =
            serde_json::from_str(r#"{ "id": "p", "owner": "Lofi Girl" }"#).expect("flat owner");
        assert_eq!(
            flat.owner.as_ref().and_then(|o| o.display_name()),
            Some("Lofi Girl"),
        );

        let nested: SportifyPlaylist = serde_json::from_str(
            r#"{ "id": "p", "owner": { "id": "u1", "display_name": "Bob" } }"#,
        )
        .expect("nested owner");
        assert_eq!(
            nested.owner.as_ref().and_then(|o| o.display_name()),
            Some("Bob"),
        );
    }

    #[test]
    fn deserializes_compact_playlist_search_shape() {
        let json = r#"{
            "spotifyId": "3jVA3AuxV9aPO07YPt3Ist",
            "title": "lost vietnamese classics",
            "imageUrl": "https://example.com/cover.jpg",
            "owner": "Felix",
            "followerCount": 42,
            "totalTracks": 11
        }"#;
        let playlist: SportifyPlaylist = serde_json::from_str(json).expect("compact playlist");
        assert_eq!(
            playlist.spotify_id().as_deref(),
            Some("3jVA3AuxV9aPO07YPt3Ist"),
        );
        assert_eq!(
            playlist.title().as_deref(),
            Some("lost vietnamese classics")
        );
        assert_eq!(
            playlist.best_thumbnail().as_deref(),
            Some("https://example.com/cover.jpg"),
        );
        assert_eq!(playlist.follower_count(), Some(42));
        assert_eq!(playlist.total_track_count(), Some(11));
    }
}
