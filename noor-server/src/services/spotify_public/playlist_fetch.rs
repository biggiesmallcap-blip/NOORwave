//! Load a whole playlist (metadata + tracks) through the web player's
//! anonymous `fetchPlaylist` operation, as the last fallback when the
//! Sportify proxies are down and no Spotify app credentials are saved.
//! Without it a playlist could show up in search but fail to open.
//!
//! Shape (`data.playlistV2`): the playlist fields `searchDesktop` returns,
//! plus `content`:
//!
//! ```json
//! "content": { "totalCount": 120, "items": [ { "itemV2": { "data": {
//!     "__typename": "Track", "uri": "spotify:track:<id>", "name": "...",
//!     "trackDuration": { "totalMilliseconds": 210000 },
//!     "playcount": "12345", "contentRating": { "label": "EXPLICIT" },
//!     "trackNumber": 3, "discNumber": 1,
//!     "artists": { "items": [ { "uri": "spotify:artist:<id>", "profile": { "name": "..." } } ] },
//!     "albumOfTrack": { "uri": "spotify:album:<id>", "name": "...",
//!         "coverArt": { "sources": [ { "url", "width", "height" } ] } }
//! } } } ] }
//! ```

use anyhow::{Result, anyhow};
use serde_json::Value;

use super::SpotifyPublicClient;
use super::playlist_search::playlist_from_data;
use crate::services::sportify::models::{
    SportifyAlbumRef, SportifyArtistRef, SportifyImage, SportifyPlaylist, SportifyTrack,
};

/// Tracks per `fetchPlaylist` page. The web player itself asks for 25; 100
/// keeps a long playlist to a handful of requests.
const PAGE_SIZE: u32 = 100;
/// Hard stop so a huge playlist can't fan out into dozens of requests.
const MAX_TRACKS: u32 = 1000;

pub async fn fetch_playlist(
    client: &SpotifyPublicClient,
    spotify_playlist_id: &str,
) -> Result<SportifyPlaylist> {
    let first = client
        .fetch_playlist_page(spotify_playlist_id, 0, PAGE_SIZE)
        .await?;
    let mut playlist = playlist_from_fetch_playlist(&first)
        .ok_or_else(|| anyhow!("fetchPlaylist: no playlist in response"))?;
    let total = total_count(&first);

    let mut offset = PAGE_SIZE;
    let mut last_page_len = page_tracks(&first).len() as u32;
    while last_page_len == PAGE_SIZE && offset < total.min(MAX_TRACKS) {
        let page = client
            .fetch_playlist_page(spotify_playlist_id, offset, PAGE_SIZE)
            .await?;
        let tracks = page_tracks(&page);
        last_page_len = tracks.len() as u32;
        playlist.tracks.extend(tracks);
        offset += PAGE_SIZE;
    }
    Ok(playlist)
}

/// Playlist metadata plus the tracks on this page.
pub fn playlist_from_fetch_playlist(body: &Value) -> Option<SportifyPlaylist> {
    let data = body.pointer("/data/playlistV2")?;
    let mut playlist = playlist_from_data(data)?;
    playlist.total_tracks = i32::try_from(total_count(body)).ok().filter(|n| *n > 0);
    playlist.tracks = page_tracks(body);
    Some(playlist)
}

fn total_count(body: &Value) -> u32 {
    body.pointer("/data/playlistV2/content/totalCount")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .unwrap_or(0)
}

fn page_tracks(body: &Value) -> Vec<SportifyTrack> {
    body.pointer("/data/playlistV2/content/items")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(track_from_item).collect())
        .unwrap_or_default()
}

fn track_from_item(item: &Value) -> Option<SportifyTrack> {
    let data = item.pointer("/itemV2/data")?;
    // Podcast episodes and local files have no Spotify track id to resolve.
    if data.get("__typename").and_then(Value::as_str) != Some("Track") {
        return None;
    }
    let id = id_from_uri(data.get("uri")?, "spotify:track:")?;

    let artists = data
        .pointer("/artists/items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|artist| SportifyArtistRef {
                    id: artist
                        .get("uri")
                        .and_then(|uri| id_from_uri(uri, "spotify:artist:")),
                    name: artist
                        .pointer("/profile/name")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    uri: artist
                        .get("uri")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();

    let album = data.get("albumOfTrack").map(|album| SportifyAlbumRef {
        id: album
            .get("uri")
            .and_then(|uri| id_from_uri(uri, "spotify:album:")),
        name: album
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
        images: album
            .pointer("/coverArt/sources")
            .and_then(Value::as_array)
            .map(|sources| sources.iter().filter_map(image_from_source).collect())
            .unwrap_or_default(),
        ..SportifyAlbumRef::default()
    });
    let thumbnail = album.as_ref().and_then(|album| {
        album
            .images
            .iter()
            .max_by_key(|image| image.width.unwrap_or(0))
            .and_then(|image| image.url.clone())
    });

    Some(SportifyTrack {
        id: Some(id.clone()),
        name: data.get("name").and_then(Value::as_str).map(str::to_string),
        artists,
        album,
        thumbnail,
        duration_ms: data
            .pointer("/trackDuration/totalMilliseconds")
            .and_then(Value::as_i64),
        explicit: data
            .pointer("/contentRating/label")
            .and_then(Value::as_str)
            .map(|label| label == "EXPLICIT"),
        track_number: data
            .get("trackNumber")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok()),
        disc_number: data
            .get("discNumber")
            .and_then(Value::as_i64)
            .and_then(|n| i32::try_from(n).ok()),
        // Spotify ships playcount as a decimal string.
        playcount: data.get("playcount").and_then(|count| {
            count
                .as_i64()
                .or_else(|| count.as_str().and_then(|s| s.parse().ok()))
        }),
        url: Some(format!("https://open.spotify.com/track/{id}")),
        ..SportifyTrack::default()
    })
}

fn id_from_uri(uri: &Value, prefix: &str) -> Option<String> {
    uri.as_str()?
        .strip_prefix(prefix)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn image_from_source(source: &Value) -> Option<SportifyImage> {
    Some(SportifyImage {
        url: Some(source.get("url")?.as_str()?.to_string()),
        width: source
            .get("width")
            .and_then(Value::as_i64)
            .and_then(|w| i32::try_from(w).ok()),
        height: source
            .get("height")
            .and_then(Value::as_i64)
            .and_then(|h| i32::try_from(h).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_user_playlist_with_tracks_and_skips_non_tracks() {
        let body = json!({ "data": { "playlistV2": {
            "__typename": "Playlist",
            "uri": "spotify:playlist:5userMadeGymMix",
            "name": "gym mix 2026",
            "description": "my lifting songs",
            "ownerV2": { "data": { "name": "Jo", "username": "jo123" } },
            "images": { "items": [ { "sources": [
                { "url": "https://mosaic.scdn.co/640/x", "width": 640, "height": 640 }
            ] } ] },
            "content": { "totalCount": 3, "items": [
                { "itemV2": { "data": {
                    "__typename": "Track",
                    "uri": "spotify:track:4cOdK2wGLETKBW3PvgPWqT",
                    "name": "Never Gonna Give You Up",
                    "trackDuration": { "totalMilliseconds": 213573 },
                    "playcount": "1234567",
                    "contentRating": { "label": "NONE" },
                    "trackNumber": 1,
                    "discNumber": 1,
                    "artists": { "items": [ {
                        "uri": "spotify:artist:0gxyHStUsqpMadRV0Di1Qt",
                        "profile": { "name": "Rick Astley" }
                    } ] },
                    "albumOfTrack": {
                        "uri": "spotify:album:6N9PS4QXF1D0OWPk0Sxtb4",
                        "name": "Whenever You Need Somebody",
                        "coverArt": { "sources": [
                            { "url": "https://i.scdn.co/image/small", "width": 64, "height": 64 },
                            { "url": "https://i.scdn.co/image/big", "width": 640, "height": 640 }
                        ] }
                    }
                } } },
                { "itemV2": { "data": { "__typename": "Episode", "uri": "spotify:episode:x" } } },
                { "itemV2": { "data": { "__typename": "LocalTrack", "uri": "spotify:local:a:b:c:1" } } }
            ] }
        } } });

        let playlist = playlist_from_fetch_playlist(&body).expect("playlist");
        assert_eq!(playlist.spotify_id().as_deref(), Some("5userMadeGymMix"));
        assert_eq!(playlist.title().as_deref(), Some("gym mix 2026"));
        assert_eq!(playlist.total_track_count(), Some(3));
        assert_eq!(playlist.tracks.len(), 1);

        let track = &playlist.tracks[0];
        assert_eq!(track.id.as_deref(), Some("4cOdK2wGLETKBW3PvgPWqT"));
        assert_eq!(track.name.as_deref(), Some("Never Gonna Give You Up"));
        assert_eq!(track.primary_artist(), Some("Rick Astley"));
        assert_eq!(track.duration_ms, Some(213573));
        assert_eq!(track.explicit, Some(false));
        assert_eq!(track.playcount, Some(1234567));
        assert_eq!(
            track.thumbnail.as_deref(),
            Some("https://i.scdn.co/image/big")
        );
        let album = track.album.as_ref().expect("album");
        assert_eq!(album.id.as_deref(), Some("6N9PS4QXF1D0OWPk0Sxtb4"));
        assert_eq!(album.name.as_deref(), Some("Whenever You Need Somebody"));
    }

    #[test]
    fn missing_playlist_is_none() {
        assert!(
            playlist_from_fetch_playlist(&json!({ "data": { "playlistV2": {
            "__typename": "NotFound"
        } } }))
            .is_none()
        );
        assert!(playlist_from_fetch_playlist(&json!({ "errors": [] })).is_none());
    }
}
