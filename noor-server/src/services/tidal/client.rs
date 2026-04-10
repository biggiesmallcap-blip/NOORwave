use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const TIDAL_API_URL: &str = "https://api.tidal.com/v1";

#[derive(Clone)]
pub struct TidalClient {
    http: reqwest::Client,
    access_token: String,
    country_code: String,
}

// ─── API Response Types ──────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TidalPaginatedResponse<T> {
    pub items: Vec<T>,
    #[serde(rename = "totalNumberOfItems")]
    pub total_number_of_items: Option<i64>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TidalTrack {
    pub id: i64,
    pub title: String,
    pub duration: i64,
    pub track_number: Option<i32>,
    pub volume_number: Option<i32>,
    pub isrc: Option<String>,
    pub artist: TidalArtist,
    pub artists: Option<Vec<TidalArtist>>,
    pub album: Option<TidalAlbumRef>,
    #[serde(rename = "audioQuality")]
    pub audio_quality: Option<String>,
    #[serde(rename = "streamReady")]
    pub stream_ready: Option<bool>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TidalAlbum {
    pub id: i64,
    pub title: String,
    #[serde(rename = "numberOfTracks")]
    pub number_of_tracks: Option<i32>,
    #[serde(rename = "numberOfVolumes")]
    pub number_of_volumes: Option<i32>,
    #[serde(rename = "releaseDate")]
    pub release_date: Option<String>,
    pub cover: Option<String>,
    pub artist: TidalArtist,
    pub artists: Option<Vec<TidalArtist>>,
    #[serde(rename = "audioQuality")]
    pub audio_quality: Option<String>,
    #[serde(rename = "type")]
    pub release_type: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TidalAlbumRef {
    pub id: i64,
    pub title: String,
    pub cover: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TidalArtist {
    pub id: i64,
    pub name: String,
    pub picture: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TidalSearchTrack {
    pub id: i64,
    pub title: String,
    pub duration: i64,
    pub artist_id: Option<i64>,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub audio_quality: Option<String>,
    pub stream_ready: Option<bool>,
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TidalSearchAlbum {
    pub id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub cover: Option<String>,
    pub artwork_url: Option<String>,
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TidalSearchArtist {
    pub id: i64,
    pub name: String,
    pub picture: Option<String>,
    pub artwork_url: Option<String>,
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TidalSearchCatalog {
    pub tracks: Vec<TidalSearchTrack>,
    pub albums: Vec<TidalSearchAlbum>,
    pub artists: Vec<TidalSearchArtist>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TidalPlaylist {
    pub uuid: String,
    pub title: String,
    pub description: Option<String>,
    #[serde(rename = "numberOfTracks")]
    pub number_of_tracks: Option<i32>,
    pub image: Option<String>,
    #[serde(rename = "squareImage")]
    pub square_image: Option<String>,
    pub creator: Option<TidalPlaylistCreator>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TidalPlaylistCreator {
    pub id: Option<i64>,
    pub name: Option<String>,
}

impl TidalClient {
    pub fn new(access_token: String, country_code: String) -> Self {
        let http = reqwest::Client::builder()
            .user_agent("TIDAL_ANDROID/1039 okhttp/3.14.9")
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client");
        Self {
            http,
            access_token,
            country_code,
        }
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.access_token)
    }

    /// Make an authenticated GET request and deserialize the response.
    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        tracing::debug!("TIDAL GET {}", url);
        let resp = self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .header("Accept-Language", "en-US")
            .send()
            .await
            .context("HTTP request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("TIDAL API error {}: {}", status, body);
        }

        let body = resp.text().await.context("Failed to read response body")?;
        tracing::debug!(
            "TIDAL response (first 200 chars): {}",
            body.char_indices()
                .nth(200)
                .map_or(&body[..], |(i, _)| &body[..i])
        );
        serde_json::from_str(&body).context(format!(
            "Failed to parse TIDAL response from {}. Body preview: {}",
            url,
            body.char_indices()
                .nth(500)
                .map_or(&body[..], |(i, _)| &body[..i])
        ))
    }

    fn artwork_url(cover_id: &str, size: i32) -> String {
        let path = cover_id.replace('-', "/");
        format!(
            "https://resources.tidal.com/images/{}/{}x{}.jpg",
            path, size, size
        )
    }

    pub fn get_artwork_url(cover: &Option<String>, size: i32) -> Option<String> {
        cover.as_ref().map(|c| Self::artwork_url(c, size))
    }

    /// Probe the current session with a small authenticated request.
    pub async fn validate_session(&self, user_id: &str) -> Result<()> {
        self.get_favorite_tracks(user_id, 1, 0).await.map(|_| ())
    }

    // ─── Favorites ─────────────────────────────────────────

    pub async fn get_favorite_tracks(
        &self,
        user_id: &str,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<FavoriteItem<TidalTrack>>> {
        let url = format!(
            "{}/users/{}/favorites/tracks?countryCode={}&limit={}&offset={}",
            TIDAL_API_URL, user_id, self.country_code, limit, offset
        );
        self.get_json(&url).await
    }

    pub async fn get_favorite_albums(
        &self,
        user_id: &str,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<FavoriteItem<TidalAlbum>>> {
        let url = format!(
            "{}/users/{}/favorites/albums?countryCode={}&limit={}&offset={}",
            TIDAL_API_URL, user_id, self.country_code, limit, offset
        );
        self.get_json(&url).await
    }

    pub async fn get_favorite_artists(
        &self,
        user_id: &str,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<FavoriteItem<TidalArtist>>> {
        let url = format!(
            "{}/users/{}/favorites/artists?countryCode={}&limit={}&offset={}",
            TIDAL_API_URL, user_id, self.country_code, limit, offset
        );
        self.get_json(&url).await
    }

    // ─── Playlists ─────────────────────────────────────────

    pub async fn get_playlists(
        &self,
        user_id: &str,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<TidalPlaylist>> {
        let url = format!(
            "{}/users/{}/playlists?countryCode={}&limit={}&offset={}",
            TIDAL_API_URL, user_id, self.country_code, limit, offset
        );
        self.get_json(&url).await
    }

    pub async fn get_playlist_tracks(
        &self,
        playlist_uuid: &str,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<TidalTrack>> {
        let url = format!(
            "{}/playlists/{}/tracks?countryCode={}&limit={}&offset={}",
            TIDAL_API_URL, playlist_uuid, self.country_code, limit, offset
        );
        self.get_json(&url).await
    }

    // ─── Album Tracks ──────────────────────────────────────

    pub async fn get_album_tracks(
        &self,
        album_id: i64,
    ) -> Result<TidalPaginatedResponse<TidalTrack>> {
        let url = format!(
            "{}/albums/{}/tracks?countryCode={}",
            TIDAL_API_URL, album_id, self.country_code
        );
        self.get_json(&url).await
    }

    // ─── Search ────────────────────────────────────────────

    pub async fn search_catalog(&self, query: &str, limit: i32) -> Result<TidalSearchCatalog> {
        let url = format!(
            "{}/search?query={}&countryCode={}&limit={}&types=TRACKS,ALBUMS,ARTISTS",
            TIDAL_API_URL,
            urlencoding::encode(query),
            self.country_code,
            limit
        );
        let payload: serde_json::Value = self.get_json(&url).await?;

        let tracks = payload
            .get("tracks")
            .and_then(|tracks| tracks.get("items"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let albums = payload
            .get("albums")
            .and_then(|albums| albums.get("items"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let artists = payload
            .get("artists")
            .and_then(|artists| artists.get("items"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();

        Ok(TidalSearchCatalog {
            tracks: tracks
                .into_iter()
                .filter_map(Self::parse_search_track)
                .collect(),
            albums: albums
                .into_iter()
                .filter_map(Self::parse_search_album)
                .collect(),
            artists: artists
                .into_iter()
                .filter_map(Self::parse_search_artist)
                .collect(),
        })
    }

    pub async fn search(&self, query: &str, limit: i32) -> Result<Vec<TidalSearchTrack>> {
        Ok(self.search_catalog(query, limit).await?.tracks)
    }

    fn parse_search_track(value: serde_json::Value) -> Option<TidalSearchTrack> {
        let object = value.as_object()?;
        let id = object.get("id")?.as_i64()?;
        let title = object.get("title")?.as_str()?.to_string();
        let duration = object.get("duration")?.as_i64()?;
        let primary_artist = object.get("artist").or_else(|| {
            object
                .get("artists")
                .and_then(serde_json::Value::as_array)
                .and_then(|a| a.first())
        });
        let artist_id = primary_artist
            .and_then(|a| a.get("id"))
            .and_then(serde_json::Value::as_i64);
        let artist_name = primary_artist
            .and_then(|a| a.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let album = object.get("album");
        let album_title = album
            .and_then(|album| album.get("title"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let artwork_url = album
            .and_then(|album| album.get("cover"))
            .and_then(serde_json::Value::as_str)
            .map(|cover| Self::artwork_url(cover, 640));
        let audio_quality = object
            .get("audioQuality")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let stream_ready = object
            .get("streamReady")
            .and_then(serde_json::Value::as_bool);
        let extra = object.clone().into_iter().collect();

        Some(TidalSearchTrack {
            id,
            title,
            duration,
            artist_id,
            artist_name,
            album_title,
            artwork_url,
            audio_quality,
            stream_ready,
            extra,
        })
    }

    fn parse_search_album(value: serde_json::Value) -> Option<TidalSearchAlbum> {
        let object = value.as_object()?;
        let id = object.get("id")?.as_i64()?;
        let title = object.get("title")?.as_str()?.to_string();
        let artist_name = object
            .get("artist")
            .and_then(|artist| artist.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                object
                    .get("artists")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|artists| artists.first())
                    .and_then(|artist| artist.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
        let cover = object
            .get("cover")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let artwork_url = cover.as_deref().map(|cover| Self::artwork_url(cover, 640));
        let extra = object.clone().into_iter().collect();

        Some(TidalSearchAlbum {
            id,
            title,
            artist_name,
            cover,
            artwork_url,
            extra,
        })
    }

    fn parse_search_artist(value: serde_json::Value) -> Option<TidalSearchArtist> {
        let object = value.as_object()?;
        let id = object.get("id")?.as_i64()?;
        let name = object.get("name")?.as_str()?.to_string();
        let picture = object
            .get("picture")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let artwork_url = picture
            .as_deref()
            .map(|picture| Self::artwork_url(picture, 640));
        let extra = object.clone().into_iter().collect();

        Some(TidalSearchArtist {
            id,
            name,
            picture,
            artwork_url,
            extra,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct FavoriteItem<T> {
    pub item: T,
    pub created: Option<String>,
}
