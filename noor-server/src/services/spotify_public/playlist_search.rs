//! Parse the playlist bucket of a `searchDesktop` response into the same
//! [`SportifyPlaylist`] rows the Sportify proxy and the Web API fallback
//! produce, so `/api/discovery/sportify/search` can use it as a drop-in
//! third source.
//!
//! Shape (`data.searchV2.playlists.items[]`):
//!
//! ```json
//! { "data": {
//!     "__typename": "Playlist",
//!     "uri": "spotify:playlist:<id>",
//!     "name": "...", "description": "...",
//!     "images": { "items": [ { "sources": [ { "url", "width", "height" } ] } ] },
//!     "ownerV2": { "data": { "name": "...", "username": "...", "uri": "spotify:user:<id>" } }
//! } }
//! ```
//!
//! Rows that are not playlists (`NotFound`, `RestrictedContent`) or carry no
//! id are dropped one by one; a bad row never blanks the page.

use serde_json::Value;

use crate::services::sportify::models::{SportifyImage, SportifyPlaylist, SportifyPlaylistOwner};

pub fn playlists_from_search_desktop(body: &Value) -> Vec<SportifyPlaylist> {
    let items = body
        .pointer("/data/searchV2/playlists/items")
        .and_then(Value::as_array);
    let Some(items) = items else {
        return Vec::new();
    };
    items.iter().filter_map(playlist_from_item).collect()
}

fn playlist_from_item(item: &Value) -> Option<SportifyPlaylist> {
    // Newer bundles wrap rows as `{ item: { data } }`; older ones as `{ data }`.
    let data = item
        .get("data")
        .or_else(|| item.pointer("/item/data"))
        .unwrap_or(item);
    if let Some(kind) = data.get("__typename").and_then(Value::as_str)
        && kind != "Playlist"
    {
        return None;
    }
    let id = data
        .get("uri")
        .and_then(Value::as_str)
        .and_then(|uri| uri.strip_prefix("spotify:playlist:"))
        .filter(|id| !id.is_empty())?
        .to_string();

    let images = data
        .pointer("/images/items")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|first| first.get("sources"))
        .and_then(Value::as_array)
        .map(|sources| {
            sources
                .iter()
                .filter_map(|src| {
                    Some(SportifyImage {
                        url: Some(src.get("url")?.as_str()?.to_string()),
                        width: src.get("width").and_then(Value::as_i64).map(|w| w as i32),
                        height: src.get("height").and_then(Value::as_i64).map(|h| h as i32),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let owner_data = data.pointer("/ownerV2/data");
    let owner = owner_data.map(|owner| SportifyPlaylistOwner::Object {
        id: owner
            .get("uri")
            .and_then(Value::as_str)
            .and_then(|uri| uri.strip_prefix("spotify:user:"))
            .or_else(|| owner.get("username").and_then(Value::as_str))
            .map(str::to_string),
        name: owner
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string),
    });

    Some(SportifyPlaylist {
        id: Some(id),
        name: data.get("name").and_then(Value::as_str).map(str::to_string),
        description: data
            .get("description")
            .and_then(Value::as_str)
            .filter(|d| !d.trim().is_empty())
            .map(str::to_string),
        images,
        owner,
        ..SportifyPlaylist::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_user_and_editorial_playlists_and_skips_unusable_rows() {
        let body = json!({
            "data": { "searchV2": { "playlists": { "totalCount": 4, "items": [
                { "data": {
                    "__typename": "Playlist",
                    "uri": "spotify:playlist:37i9dQZF1DX8Uebhn9wzrS",
                    "name": "Chill Lofi Study Beats",
                    "description": "",
                    "images": { "items": [ { "sources": [
                        { "url": "https://i.scdn.co/image/a", "width": 300, "height": 300 },
                        { "url": "https://i.scdn.co/image/b", "width": 640, "height": 640 }
                    ] } ] },
                    "ownerV2": { "data": { "name": "Spotify", "uri": "spotify:user:spotify" } }
                } },
                { "item": { "data": {
                    "__typename": "Playlist",
                    "uri": "spotify:playlist:5userMadeGymMix",
                    "name": "gym mix 2026",
                    "description": "my lifting songs",
                    "images": { "items": [] },
                    "ownerV2": { "data": { "name": "Jo", "username": "jo123" } }
                } } },
                { "data": { "__typename": "NotFound" } },
                { "data": { "__typename": "Playlist", "name": "no uri" } }
            ] } } }
        });

        let rows = playlists_from_search_desktop(&body);
        assert_eq!(rows.len(), 2);

        assert_eq!(
            rows[0].spotify_id().as_deref(),
            Some("37i9dQZF1DX8Uebhn9wzrS")
        );
        assert_eq!(
            rows[0].best_thumbnail().as_deref(),
            Some("https://i.scdn.co/image/b")
        );
        assert_eq!(rows[0].description, None);
        assert_eq!(
            rows[0].owner.as_ref().and_then(|o| o.display_name()),
            Some("Spotify")
        );

        assert_eq!(rows[1].spotify_id().as_deref(), Some("5userMadeGymMix"));
        assert_eq!(rows[1].title().as_deref(), Some("gym mix 2026"));
        assert_eq!(
            rows[1].owner.as_ref().and_then(|o| o.display_name()),
            Some("Jo")
        );
        assert!(rows[1].images.is_empty());
    }

    #[test]
    fn missing_playlist_bucket_yields_no_rows() {
        assert!(playlists_from_search_desktop(&json!({ "data": { "searchV2": {} } })).is_empty());
        assert!(playlists_from_search_desktop(&json!({ "errors": [] })).is_empty());
    }
}
