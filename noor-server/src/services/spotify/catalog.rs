use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;

use crate::db::Database;
use crate::services::sportify::models::{
    SportifyAlbumRef, SportifyArtistRef, SportifyExternalIds, SportifyImage, SportifyPlaylist,
    SportifyPlaylistOwner, SportifySearchResults, SportifyTrack,
};

use super::auth;

const SPOTIFY_API_BASE: &str = "https://api.spotify.com/v1";

#[derive(Debug, Deserialize, Default)]
struct SpotifyImageDto {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    width: Option<i32>,
    #[serde(default)]
    height: Option<i32>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyOwnerDto {
    #[serde(default, alias = "displayName")]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyFollowersDto {
    #[serde(default)]
    total: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyTracksSummaryDto {
    #[serde(default)]
    total: Option<i32>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyPlaylistSearchDto {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    images: Vec<SpotifyImageDto>,
    #[serde(default)]
    owner: Option<SpotifyOwnerDto>,
    #[serde(default)]
    followers: Option<SpotifyFollowersDto>,
    #[serde(default)]
    tracks: Option<SpotifyTracksSummaryDto>,
    #[serde(default)]
    external_urls: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifySearchResponse {
    #[serde(default)]
    playlists: Option<SpotifyPaging<SpotifyPlaylistSearchDto>>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyPaging<T> {
    #[serde(default)]
    items: Vec<Option<T>>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyArtistDto {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyAlbumDto {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    images: Vec<SpotifyImageDto>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    total_tracks: Option<i32>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyExternalIdsDto {
    #[serde(default)]
    isrc: Option<String>,
    #[serde(default)]
    ean: Option<String>,
    #[serde(default)]
    upc: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyTrackDto {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    artists: Vec<SpotifyArtistDto>,
    #[serde(default)]
    album: Option<SpotifyAlbumDto>,
    #[serde(default)]
    duration_ms: Option<i64>,
    #[serde(default)]
    explicit: Option<bool>,
    #[serde(default)]
    track_number: Option<i32>,
    #[serde(default)]
    disc_number: Option<i32>,
    #[serde(default)]
    preview_url: Option<String>,
    #[serde(default)]
    popularity: Option<i32>,
    #[serde(default)]
    external_ids: Option<SpotifyExternalIdsDto>,
    #[serde(default)]
    external_urls: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyPlaylistTrackItemDto {
    #[serde(default)]
    track: Option<SpotifyTrackDto>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyPlaylistTracksDto {
    #[serde(default)]
    total: Option<i32>,
    #[serde(default)]
    items: Vec<SpotifyPlaylistTrackItemDto>,
}

#[derive(Debug, Deserialize, Default)]
struct SpotifyPlaylistDetailDto {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    images: Vec<SpotifyImageDto>,
    #[serde(default)]
    owner: Option<SpotifyOwnerDto>,
    #[serde(default)]
    followers: Option<SpotifyFollowersDto>,
    #[serde(default)]
    tracks: Option<SpotifyPlaylistTracksDto>,
    #[serde(default)]
    external_urls: HashMap<String, String>,
}

struct SpotifyCatalogClient {
    http: Client,
    token: String,
}

impl SpotifyCatalogClient {
    fn new(http: Client, token: String) -> Self {
        Self { http, token }
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let response = self
            .http
            .get(format!("{}{}", SPOTIFY_API_BASE, path))
            .bearer_auth(&self.token)
            .query(query)
            .send()
            .await
            .with_context(|| format!("spotify catalog request failed: {}", path))?;

        let status = response.status();
        let body = response.text().await.context("read spotify catalog body")?;
        if !status.is_success() {
            anyhow::bail!(
                "spotify catalog {} returned HTTP {}: {}",
                path,
                status,
                truncate_for_log(&body)
            );
        }

        serde_json::from_str(&body).with_context(|| format!("parse spotify catalog {}", path))
    }

    async fn search_playlists(
        &self,
        query: &str,
        limit: u32,
        offset: u32,
    ) -> Result<SportifySearchResults> {
        let payload: SpotifySearchResponse = self
            .get_json(
                "/search",
                &[
                    ("q", query.to_string()),
                    ("type", "playlist".to_string()),
                    ("limit", limit.clamp(1, 50).to_string()),
                    ("offset", offset.to_string()),
                ],
            )
            .await?;

        Ok(SportifySearchResults {
            playlists: payload
                .playlists
                .map(|page| {
                    page.items
                        .into_iter()
                        .flatten()
                        .map(playlist_from_search)
                        .collect()
                })
                .unwrap_or_default(),
            ..SportifySearchResults::default()
        })
    }

    async fn playlist(&self, spotify_id: &str) -> Result<SportifyPlaylist> {
        let payload: SpotifyPlaylistDetailDto = self
            .get_json(
                &format!("/playlists/{}", spotify_id),
                &[(
                    "fields",
                    [
                        "id,name,description,images,owner(display_name),followers(total)",
                        "tracks(total,items(track(id,name,artists(id,name),album(id,name,images,release_date,total_tracks),duration_ms,explicit,track_number,disc_number,preview_url,popularity,external_ids,external_urls)))",
                    ]
                    .join(","),
                )],
            )
            .await?;

        Ok(playlist_from_detail(payload))
    }
}

pub async fn search_playlists_from_saved_credentials(
    db: &Database,
    query: &str,
    limit: u32,
    offset: u32,
) -> Result<SportifySearchResults> {
    let client = catalog_client_from_saved_credentials(db).await?;
    client.search_playlists(query, limit, offset).await
}

pub async fn playlist_from_saved_credentials(
    db: &Database,
    spotify_id: &str,
) -> Result<SportifyPlaylist> {
    let client = catalog_client_from_saved_credentials(db).await?;
    client.playlist(spotify_id).await
}

async fn catalog_client_from_saved_credentials(db: &Database) -> Result<SpotifyCatalogClient> {
    let creds = db
        .with_conn(|conn| Ok(auth::load_credentials(conn).ok().flatten()))?
        .context("spotify credentials not configured")?;
    let http = Client::new();
    let token = auth::fetch_app_token(&http, &creds)
        .await
        .context("fetch spotify catalog token")?;
    Ok(SpotifyCatalogClient::new(http, token.access_token))
}

fn playlist_from_search(dto: SpotifyPlaylistSearchDto) -> SportifyPlaylist {
    let total_tracks = dto.tracks.and_then(|tracks| tracks.total);
    let owner = dto.owner.and_then(|owner| {
        owner
            .display_name
            .map(|name| SportifyPlaylistOwner::Object {
                id: None,
                name: Some(name),
            })
    });
    SportifyPlaylist {
        id: dto.id,
        name: dto.name,
        description: dto.description,
        images: dto.images.into_iter().map(image_from_dto).collect(),
        owner,
        followers: dto.followers.and_then(|followers| followers.total),
        total_tracks,
        url: dto.external_urls.get("spotify").cloned(),
        ..SportifyPlaylist::default()
    }
}

fn playlist_from_detail(dto: SpotifyPlaylistDetailDto) -> SportifyPlaylist {
    let (total_tracks, tracks) = dto
        .tracks
        .map(|tracks| {
            (
                tracks.total,
                tracks
                    .items
                    .into_iter()
                    .filter_map(|item| item.track)
                    .map(track_from_dto)
                    .collect(),
            )
        })
        .unwrap_or((None, Vec::new()));
    let owner = dto.owner.and_then(|owner| {
        owner
            .display_name
            .map(|name| SportifyPlaylistOwner::Object {
                id: None,
                name: Some(name),
            })
    });

    SportifyPlaylist {
        id: dto.id,
        name: dto.name,
        description: dto.description,
        images: dto.images.into_iter().map(image_from_dto).collect(),
        owner,
        followers: dto.followers.and_then(|followers| followers.total),
        total_tracks,
        url: dto.external_urls.get("spotify").cloned(),
        tracks,
        ..SportifyPlaylist::default()
    }
}

fn track_from_dto(dto: SpotifyTrackDto) -> SportifyTrack {
    let album = dto.album.map(|album| SportifyAlbumRef {
        id: album.id,
        name: album.name,
        images: album.images.into_iter().map(image_from_dto).collect(),
        release_date: album.release_date,
        total_tracks: album.total_tracks,
    });

    SportifyTrack {
        id: dto.id,
        name: dto.name,
        artists: dto
            .artists
            .into_iter()
            .map(|artist| SportifyArtistRef {
                id: artist.id,
                name: artist.name,
                uri: None,
            })
            .collect(),
        thumbnail: album
            .as_ref()
            .and_then(|album| {
                album
                    .images
                    .iter()
                    .max_by_key(|image| image.width.unwrap_or(0))
            })
            .and_then(|image| image.url.clone()),
        album,
        duration_ms: dto.duration_ms,
        explicit: dto.explicit,
        track_number: dto.track_number,
        disc_number: dto.disc_number,
        preview_url: dto.preview_url,
        popularity: dto.popularity,
        external_ids: dto.external_ids.map(|ids| SportifyExternalIds {
            isrc: ids.isrc,
            ean: ids.ean,
            upc: ids.upc,
        }),
        external_urls: dto.external_urls,
        ..SportifyTrack::default()
    }
}

fn image_from_dto(dto: SpotifyImageDto) -> SportifyImage {
    SportifyImage {
        url: dto.url,
        width: dto.width,
        height: dto.height,
    }
}

fn truncate_for_log(s: &str) -> String {
    if s.len() > 256 {
        format!("{}...", &s[..256])
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_search_playlist_total_from_nested_tracks() {
        let payload = SpotifyPlaylistSearchDto {
            id: Some("playlist-1".to_string()),
            name: Some("Lofi".to_string()),
            tracks: Some(SpotifyTracksSummaryDto { total: Some(42) }),
            owner: Some(SpotifyOwnerDto {
                display_name: Some("Spotify".to_string()),
            }),
            ..SpotifyPlaylistSearchDto::default()
        };

        let playlist = playlist_from_search(payload);

        assert_eq!(playlist.spotify_id().as_deref(), Some("playlist-1"));
        assert_eq!(playlist.title().as_deref(), Some("Lofi"));
        assert_eq!(playlist.total_track_count(), Some(42));
        assert_eq!(
            playlist
                .owner
                .as_ref()
                .and_then(|owner| owner.display_name()),
            Some("Spotify")
        );
    }

    #[test]
    fn maps_search_response_skips_null_playlist_rows() {
        let payload = SpotifySearchResponse {
            playlists: Some(SpotifyPaging {
                items: vec![
                    None,
                    Some(SpotifyPlaylistSearchDto {
                        id: Some("playlist-1".to_string()),
                        name: Some("Lofi".to_string()),
                        ..SpotifyPlaylistSearchDto::default()
                    }),
                ],
            }),
        };

        let playlists: Vec<SportifyPlaylist> = payload
            .playlists
            .map(|page| {
                page.items
                    .into_iter()
                    .flatten()
                    .map(playlist_from_search)
                    .collect()
            })
            .unwrap_or_default();

        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].spotify_id().as_deref(), Some("playlist-1"));
    }

    #[test]
    fn maps_playlist_detail_tracks_for_tidal_resolution() {
        let payload = SpotifyPlaylistDetailDto {
            id: Some("playlist-1".to_string()),
            tracks: Some(SpotifyPlaylistTracksDto {
                total: Some(1),
                items: vec![SpotifyPlaylistTrackItemDto {
                    track: Some(SpotifyTrackDto {
                        id: Some("track-1".to_string()),
                        name: Some("Song".to_string()),
                        artists: vec![SpotifyArtistDto {
                            id: Some("artist-1".to_string()),
                            name: Some("Artist".to_string()),
                        }],
                        album: Some(SpotifyAlbumDto {
                            id: Some("album-1".to_string()),
                            name: Some("Album".to_string()),
                            images: vec![SpotifyImageDto {
                                url: Some("https://i.scdn.co/image/test".to_string()),
                                width: Some(640),
                                height: Some(640),
                            }],
                            ..SpotifyAlbumDto::default()
                        }),
                        external_ids: Some(SpotifyExternalIdsDto {
                            isrc: Some("USABC1234567".to_string()),
                            ..SpotifyExternalIdsDto::default()
                        }),
                        ..SpotifyTrackDto::default()
                    }),
                }],
            }),
            ..SpotifyPlaylistDetailDto::default()
        };

        let playlist = playlist_from_detail(payload);
        let track = playlist.tracks.first().expect("track");

        assert_eq!(playlist.total_track_count(), Some(1));
        assert_eq!(track.id.as_deref(), Some("track-1"));
        assert_eq!(track.primary_artist(), Some("Artist"));
        assert_eq!(
            track.album.as_ref().and_then(|album| album.name.as_deref()),
            Some("Album")
        );
        assert_eq!(
            track
                .external_ids
                .as_ref()
                .and_then(|ids| ids.isrc.as_deref()),
            Some("USABC1234567")
        );
        assert_eq!(
            track.best_thumbnail().as_deref(),
            Some("https://i.scdn.co/image/test")
        );
    }
}
