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
    #[allow(dead_code)]
    pub limit: Option<i64>,
    #[allow(dead_code)]
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
    pub artist_picture: Option<String>,
    pub album_title: Option<String>,
    pub album_id: Option<i64>,
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
        crate::services::tidal::backoff::global().check()?;

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
            crate::services::tidal::backoff::global().classify(status.as_u16(), &body);
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

    pub async fn search_playlists(&self, query: &str, limit: i32, offset: i32) -> Result<Vec<TidalPlaylist>> {
        let url = format!(
            "{}/search?query={}&countryCode={}&limit={}&offset={}&types=PLAYLISTS",
            TIDAL_API_URL,
            urlencoding::encode(query),
            self.country_code,
            limit,
            offset.max(0),
        );
        let payload: serde_json::Value = self.get_json(&url).await?;
        let items = payload
            .get("playlists")
            .and_then(|p| p.get("items"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(items
            .into_iter()
            .filter_map(|v| serde_json::from_value::<TidalPlaylist>(v).ok())
            .collect())
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

    // ─── Artist Discography ────────────────────────────────

    pub async fn get_track(&self, track_id: i64) -> Result<TidalTrack> {
        let url = format!(
            "{}/tracks/{}?countryCode={}",
            TIDAL_API_URL, track_id, self.country_code
        );
        self.get_json(&url).await
    }

    pub async fn get_artist_albums(
        &self,
        artist_id: i64,
        limit: i32,
        offset: i32,
        filter: Option<&str>,
    ) -> Result<TidalPaginatedResponse<TidalAlbum>> {
        let filter_param = filter.map(|f| format!("&filter={f}")).unwrap_or_default();
        let url = format!(
            "{}/artists/{}/albums?countryCode={}&limit={}&offset={}{}",
            TIDAL_API_URL, artist_id, self.country_code, limit, offset, filter_param
        );
        self.get_json(&url).await
    }

    pub async fn get_artist_top_tracks(
        &self,
        artist_id: i64,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<TidalTrack>> {
        let url = format!(
            "{}/artists/{}/toptracks?countryCode={}&limit={}&offset={}",
            TIDAL_API_URL, artist_id, self.country_code, limit, offset
        );
        self.get_json(&url).await
    }

    // ─── Search ────────────────────────────────────────────

    pub async fn search_catalog(&self, query: &str, limit: i32, offset: i32) -> Result<TidalSearchCatalog> {
        let url = format!(
            "{}/search?query={}&countryCode={}&limit={}&offset={}&types=TRACKS,ALBUMS,ARTISTS",
            TIDAL_API_URL,
            urlencoding::encode(query),
            self.country_code,
            limit,
            offset.max(0)
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
        Ok(self.search_catalog(query, limit, 0).await?.tracks)
    }

    /// Fetch Tidal editorial "Top Tracks" for the user's region.
    ///
    /// Tidal's public-ish editorial endpoints (e.g. `featured/{path}/tracks`)
    /// vary by region and aren't documented for the API surface we use.
    /// TODO(charts): wire this up once the canonical endpoint is confirmed —
    /// candidate paths include `pages/explore`, `featured/new/tracks`, and
    /// `editorial/charts/tracks`. Until then we attempt the most likely
    /// `pages/genre/all/tracks` shape and return an empty list with a logged
    /// warning if it fails so the route can fall back to Last.fm.
    pub async fn get_editorial_top_tracks(&self, limit: i32) -> Result<Vec<TidalSearchTrack>> {
        let url = format!(
            "{}/pages/genre/all/tracks?countryCode={}&limit={}&deviceType=DESKTOP",
            TIDAL_API_URL, self.country_code, limit
        );
        let payload: serde_json::Value = match self.get_json(&url).await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    "TIDAL editorial chart fetch failed (endpoint not confirmed): {}",
                    err
                );
                return Ok(Vec::new());
            }
        };
        let tracks = payload
            .get("items")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(tracks
            .into_iter()
            .filter_map(Self::parse_search_track)
            .collect())
    }

    /// Fetch the authenticated user's "My Mixes" page from TIDAL.
    ///
    /// Endpoint: `pages/my_collection_my_mixes` — undocumented private API
    /// used by the TIDAL web client. Stable enough in practice, but if TIDAL
    /// breaks the shape we surface an empty list with a `warn` rather than
    /// erroring the whole home page (acceptable risk per plan).
    ///
    /// Response shape: `{ rows: [{ modules: [{ pagedList: { items: [...] } }] }] }`
    /// where each item is a mix `{ id, title, subTitle, mixType, images: {...} }`.
    pub async fn get_my_mixes(&self) -> Result<Vec<TidalMix>> {
        let url = format!(
            "{}/pages/my_collection_my_mixes?countryCode={}&deviceType=BROWSER&locale=en_US",
            TIDAL_API_URL, self.country_code
        );
        let payload: serde_json::Value = match self.get_json(&url).await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!("TIDAL my_mixes fetch failed: {}", err);
                return Ok(Vec::new());
            }
        };
        let mixes = Self::parse_my_mixes(&payload);
        if mixes.is_empty() {
            // Surface the actual response shape so we can debug why parsing
            // returned nothing (vs. the user genuinely having no mixes).
            // Top-level keys + rows[].modules summary, no nested item dump.
            let top_keys: Vec<&str> = payload
                .as_object()
                .map(|o| o.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let rows_summary: Vec<String> = payload
                .get("rows")
                .and_then(serde_json::Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .enumerate()
                        .map(|(i, row)| {
                            let modules = row
                                .get("modules")
                                .and_then(serde_json::Value::as_array)
                                .map(|m| {
                                    m.iter()
                                        .map(|module| {
                                            let kind = module
                                                .get("type")
                                                .and_then(serde_json::Value::as_str)
                                                .unwrap_or("?");
                                            let item_count = module
                                                .get("pagedList")
                                                .and_then(|p| p.get("items"))
                                                .and_then(serde_json::Value::as_array)
                                                .map(|a| a.len())
                                                .unwrap_or(0);
                                            format!("{kind}({item_count})")
                                        })
                                        .collect::<Vec<_>>()
                                        .join(",")
                                })
                                .unwrap_or_else(|| "no-modules".into());
                            format!("row{i}=[{modules}]")
                        })
                        .collect()
                })
                .unwrap_or_default();
            tracing::warn!(
                target: "noor.tidal.mixes",
                "parse_my_mixes returned 0 — top_keys={:?}, rows_summary={:?}, raw_len={}",
                top_keys,
                rows_summary,
                payload.to_string().len()
            );
        }
        Ok(mixes)
    }

    /// Fetch the items inside a TIDAL mix. The endpoint is the only public
    /// way to play a mix server-side, since mixes don't have a stable track
    /// set we could import once.
    pub async fn get_mix_tracks(&self, mix_id: &str) -> Result<Vec<TidalTrack>> {
        let url = format!(
            "{}/mixes/{}/items?countryCode={}&limit=100",
            TIDAL_API_URL, mix_id, self.country_code
        );
        let payload: serde_json::Value = self.get_json(&url).await?;
        Ok(Self::parse_mix_track_items(&payload))
    }

    fn parse_mix_track_items(payload: &serde_json::Value) -> Vec<TidalTrack> {
        let Some(items) = payload.get("items").and_then(serde_json::Value::as_array) else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|wrapper| {
                // Some endpoints return `{ item: TidalTrack, type: "track" }`,
                // others return the track directly. Try both.
                let item = wrapper.get("item").unwrap_or(wrapper);
                serde_json::from_value::<TidalTrack>(item.clone()).ok()
            })
            .collect()
    }

    fn parse_my_mixes(payload: &serde_json::Value) -> Vec<TidalMix> {
        let mut out = Vec::new();

        // Shape 1 (web client): rows[].modules[].pagedList.items[]
        if let Some(rows) = payload.get("rows").and_then(serde_json::Value::as_array) {
            for row in rows {
                let Some(modules) = row.get("modules").and_then(serde_json::Value::as_array) else {
                    continue;
                };
                for module in modules {
                    // Try pagedList.items first, then plain items[].
                    let items = module
                        .get("pagedList")
                        .and_then(|p| p.get("items"))
                        .and_then(serde_json::Value::as_array)
                        .or_else(|| module.get("items").and_then(serde_json::Value::as_array));
                    let Some(items) = items else { continue };
                    for item in items {
                        if let Some(mix) = Self::parse_mix_item(item) {
                            out.push(mix);
                        }
                    }
                }
            }
        }

        // Shape 2 (older TIDAL): top-level items[]
        if out.is_empty() {
            if let Some(items) = payload.get("items").and_then(serde_json::Value::as_array) {
                for item in items {
                    if let Some(mix) = Self::parse_mix_item(item) {
                        out.push(mix);
                    }
                }
            }
        }

        // Shape 3 (some regions): top-level mixes[]
        if out.is_empty() {
            if let Some(items) = payload.get("mixes").and_then(serde_json::Value::as_array) {
                for item in items {
                    if let Some(mix) = Self::parse_mix_item(item) {
                        out.push(mix);
                    }
                }
            }
        }

        out
    }

    fn parse_mix_item(item: &serde_json::Value) -> Option<TidalMix> {
        let obj = item.as_object()?;
        // TIDAL mix ids are short alphanumeric strings, not numeric like tracks.
        let id = obj.get("id")?.as_str()?.to_string();
        let title = obj.get("title")?.as_str()?.to_string();
        let sub_title = obj
            .get("subTitle")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let mix_type = obj
            .get("mixType")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let image_url = Self::pick_mix_image(obj.get("images"));
        Some(TidalMix {
            id,
            title,
            sub_title,
            image_url,
            mix_type,
        })
    }

    /// TIDAL mix `images` ships in two shapes depending on the page version:
    ///   1. dict keyed by size: `{"SQUARE": {"url": "..."}, "MEDIUM": {...}}`
    ///   2. dict keyed by image kind: `{"640": {"imageId": "..."}, ...}`
    ///   3. flat array: `[{"url": "..."}]`
    /// We accept all three. For shape 2 we feed `imageId` through the standard
    /// `resources.tidal.com` artwork builder.
    fn pick_mix_image(images: Option<&serde_json::Value>) -> Option<String> {
        let images = images?;
        // Shape 1 / 2: object
        if let Some(obj) = images.as_object() {
            // Prefer SQUARE > MEDIUM > LARGE > whatever-comes-first
            for key in ["SQUARE", "MEDIUM", "LARGE"] {
                if let Some(v) = obj.get(key) {
                    if let Some(u) = direct_url_or_image_id(v) {
                        return Some(u);
                    }
                }
            }
            for v in obj.values() {
                if let Some(u) = direct_url_or_image_id(v) {
                    return Some(u);
                }
            }
        }
        // Shape 3: array
        if let Some(arr) = images.as_array() {
            for v in arr {
                if let Some(u) = direct_url_or_image_id(v) {
                    return Some(u);
                }
            }
        }
        None
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
        let artist_picture = primary_artist
            .and_then(|a| a.get("picture"))
            .and_then(serde_json::Value::as_str)
            .map(|p| Self::artwork_url(p, 640));
        let album = object.get("album");
        let album_title = album
            .and_then(|album| album.get("title"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let album_id = album
            .and_then(|album| album.get("id"))
            .and_then(serde_json::Value::as_i64);
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
            artist_picture,
            album_title,
            album_id,
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
    #[allow(dead_code)]
    pub created: Option<String>,
}

/// One TIDAL mix card (Daily Discovery, My Mix #N, Master Mix, etc).
/// Returned by [`TidalClient::get_my_mixes`].
#[derive(Debug, Clone, Serialize)]
pub struct TidalMix {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// e.g. `"DAILY_DISCOVERY"`, `"PERSONAL"`, `"MASTER_ARTIST"`. Free-form
    /// — TIDAL adds new types over time. Used for icon/category hints in the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mix_type: Option<String>,
}

/// Pull a renderable URL out of the various shapes TIDAL returns for an
/// `images` entry: direct `url`, or an `imageId` we can route through the
/// standard `resources.tidal.com` artwork builder.
fn direct_url_or_image_id(v: &serde_json::Value) -> Option<String> {
    if let Some(u) = v
        .get("url")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Some(u.to_string());
    }
    if let Some(id) = v
        .get("imageId")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return Some(TidalClient::artwork_url(id, 640));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Round-trip the documented `pages/my_collection_my_mixes` shape through
    /// the parser. Asserts: every mix has non-empty id/title/image_url, the
    /// mix_type passes through, and rows/modules nesting is walked.
    #[test]
    fn parse_my_mixes_extracts_full_set() {
        let payload = json!({
            "rows": [
                {
                    "modules": [
                        {
                            "pagedList": {
                                "items": [
                                    {
                                        "id": "0123abcd",
                                        "title": "My Daily Discovery",
                                        "subTitle": "Updated daily",
                                        "mixType": "DAILY_DISCOVERY",
                                        "images": {
                                            "SQUARE": { "url": "https://img.tidal.com/sq.jpg" },
                                            "MEDIUM": { "url": "https://img.tidal.com/md.jpg" }
                                        }
                                    },
                                    {
                                        "id": "0456beef",
                                        "title": "My Mix 1",
                                        "subTitle": "From your loved tracks",
                                        "mixType": "PERSONAL",
                                        "images": {
                                            "640": { "imageId": "abc-def-ghi" }
                                        }
                                    }
                                ]
                            }
                        }
                    ]
                }
            ]
        });
        let mixes = TidalClient::parse_my_mixes(&payload);
        assert_eq!(mixes.len(), 2, "expected 2 mixes parsed from fixture");
        assert!(mixes.iter().all(|m| !m.id.is_empty()));
        assert!(mixes.iter().all(|m| !m.title.is_empty()));
        assert!(
            mixes
                .iter()
                .all(|m| m.image_url.as_deref().is_some_and(|u| !u.is_empty())),
            "every mix must surface an image url"
        );
        assert_eq!(mixes[0].mix_type.as_deref(), Some("DAILY_DISCOVERY"));
        assert_eq!(
            mixes[0].image_url.as_deref(),
            Some("https://img.tidal.com/sq.jpg")
        );
        // Shape-2 imageId routed through the resources.tidal.com builder.
        assert!(
            mixes[1]
                .image_url
                .as_deref()
                .unwrap()
                .starts_with("https://resources.tidal.com/images/abc/def/ghi/"),
            "imageId should be routed through artwork_url; got {:?}",
            mixes[1].image_url
        );
    }

    /// An empty / malformed payload must yield an empty list, not panic.
    #[test]
    fn parse_my_mixes_handles_missing_rows() {
        assert!(TidalClient::parse_my_mixes(&json!({})).is_empty());
        assert!(TidalClient::parse_my_mixes(&json!({"rows": []})).is_empty());
        assert!(TidalClient::parse_my_mixes(&json!({"rows": [{"modules": []}]})).is_empty());
    }
}
