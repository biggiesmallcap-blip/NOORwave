use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use tokio::sync::Semaphore;

const TIDAL_API_URL: &str = "https://api.tidal.com/v1";

/// Cap on concurrent in-flight TIDAL catalog requests. A single user session
/// can otherwise fan out search + playlist + playback enrichment in parallel
/// and trip TIDAL's per-second rate limit. 4 is empirically enough for
/// snappy UI without bursting; raise if catalog browsing ever feels gated.
const MAX_INFLIGHT_REQUESTS: usize = 4;

static REQUEST_LIMITER: OnceLock<Semaphore> = OnceLock::new();

fn request_limiter() -> &'static Semaphore {
    REQUEST_LIMITER.get_or_init(|| Semaphore::new(MAX_INFLIGHT_REQUESTS))
}

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
    #[serde(rename = "trackNumber")]
    pub track_number: Option<i32>,
    #[serde(rename = "volumeNumber")]
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

/// Subset of TIDAL's `/artists/{id}/videos` item shape we render in the rail.
/// Other fields (audioQuality, explicit, releaseDate, etc.) are flattened
/// into `extra` so deserialization stays forwards-compatible.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TidalArtistVideo {
    pub id: i64,
    pub title: String,
    /// Seconds. Convert to ms at the API boundary.
    pub duration: i64,
    #[serde(rename = "imageId")]
    pub image_id: Option<String>,
    pub artist: Option<TidalArtist>,
    pub album: Option<TidalAlbumRef>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Response shape of `/artists/{id}/bio`. TIDAL marks up `text` with
/// `[wimpLink]` tags; the frontend strips them. `summary` is plain prose.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TidalArtistBio {
    pub source: Option<String>,
    pub summary: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TidalSearchAlbum {
    pub id: i64,
    pub title: String,
    pub artist_name: Option<String>,
    pub cover: Option<String>,
    pub artwork_url: Option<String>,
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TidalSearchArtist {
    pub id: i64,
    pub name: String,
    pub picture: Option<String>,
    pub artwork_url: Option<String>,
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TidalSearchVideo {
    pub id: i64,
    pub title: String,
    pub duration: Option<i64>,
    pub artist_id: Option<i64>,
    pub artist_name: Option<String>,
    pub album_id: Option<i64>,
    pub artwork_url: Option<String>,
    pub quality: Option<String>,
    pub explicit: Option<bool>,
    pub r#type: String,
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct TidalSearchCatalog {
    pub tracks: Vec<TidalSearchTrack>,
    pub albums: Vec<TidalSearchAlbum>,
    pub artists: Vec<TidalSearchArtist>,
    pub videos: Vec<TidalSearchVideo>,
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
    /// Build a TIDAL-tuned `reqwest::Client`. Call once at boot and stash on
    /// `AppState`; per-request `TidalClient` instances should use
    /// `with_http` to reuse it instead of paying TLS pool setup repeatedly.
    pub fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .user_agent("TIDAL_ANDROID/1039 okhttp/3.14.9")
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build TIDAL HTTP client")
    }

    /// Construct a `TidalClient` with a caller-provided shared HTTP client.
    /// Preferred for request-scoped use — see `AppState::tidal_http_client`.
    pub fn with_http(http: reqwest::Client, access_token: String, country_code: String) -> Self {
        Self {
            http,
            access_token,
            country_code,
        }
    }

    /// Convenience constructor that builds a fresh HTTP client. Prefer
    /// `with_http` when an `AppState`-level client is available — building a
    /// client per call pays the TLS-pool setup repeatedly.
    #[cfg(test)]
    pub fn new(access_token: String, country_code: String) -> Self {
        Self::with_http(Self::build_http_client(), access_token, country_code)
    }

    fn auth_header(&self) -> String {
        format!("Bearer {}", self.access_token)
    }

    /// Make an authenticated GET request and deserialize the response.
    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        crate::services::tidal::backoff::global().check()?;

        let _permit = request_limiter()
            .acquire()
            .await
            .context("TIDAL request limiter closed")?;

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
            let retry_after = crate::services::tidal::backoff::retry_after_secs(resp.headers());
            let body = resp.text().await.unwrap_or_default();
            crate::services::tidal::backoff::global().classify(status.as_u16(), &body, retry_after);
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

    fn favorite_url(&self, user_id: &str, kind: &str, limit: i32, offset: i32) -> String {
        format!(
            "{}/users/{}/favorites/{}?countryCode={}&limit={}&offset={}&order=DATE&orderDirection=DESC",
            TIDAL_API_URL, user_id, kind, self.country_code, limit, offset
        )
    }

    pub(crate) fn favorite_tracks_url(&self, user_id: &str, limit: i32, offset: i32) -> String {
        self.favorite_url(user_id, "tracks", limit, offset)
    }

    pub(crate) fn favorite_albums_url(&self, user_id: &str, limit: i32, offset: i32) -> String {
        self.favorite_url(user_id, "albums", limit, offset)
    }

    pub(crate) fn favorite_artists_url(&self, user_id: &str, limit: i32, offset: i32) -> String {
        self.favorite_url(user_id, "artists", limit, offset)
    }

    pub async fn get_favorite_tracks(
        &self,
        user_id: &str,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<FavoriteItem<TidalTrack>>> {
        let url = self.favorite_tracks_url(user_id, limit, offset);
        self.get_json(&url).await
    }

    pub async fn get_favorite_albums(
        &self,
        user_id: &str,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<FavoriteItem<TidalAlbum>>> {
        let url = self.favorite_albums_url(user_id, limit, offset);
        self.get_json(&url).await
    }

    pub async fn get_favorite_artists(
        &self,
        user_id: &str,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<FavoriteItem<TidalArtist>>> {
        let url = self.favorite_artists_url(user_id, limit, offset);
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

    pub async fn search_playlists(
        &self,
        query: &str,
        limit: i32,
        offset: i32,
    ) -> Result<Vec<TidalPlaylist>> {
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

    /// Fetch the artist's own profile record — includes the canonical
    /// `picture` URL the web client uses for the artist hero. Used by the
    /// view-time fallback when our local DB row has no `photo_url`.
    pub async fn get_artist(&self, artist_id: i64) -> Result<TidalArtist> {
        let url = format!(
            "{}/artists/{}?countryCode={}",
            TIDAL_API_URL, artist_id, self.country_code
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

    /// Music videos for an artist. TIDAL returns the same video shape as
    /// `/videos/{id}` here — we deserialize the subset we render in the rail.
    pub async fn get_artist_videos(
        &self,
        artist_id: i64,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<TidalArtistVideo>> {
        let url = format!(
            "{}/artists/{}/videos?countryCode={}&limit={}&offset={}",
            TIDAL_API_URL, artist_id, self.country_code, limit, offset
        );
        self.get_json(&url).await
    }

    /// Long-form biography text. TIDAL returns `text` (with [wimpLink] markup)
    /// + `summary`. Caller is responsible for any markup stripping.
    pub async fn get_artist_bio(&self, artist_id: i64) -> Result<TidalArtistBio> {
        let url = format!(
            "{}/artists/{}/bio?countryCode={}",
            TIDAL_API_URL, artist_id, self.country_code
        );
        self.get_json(&url).await
    }

    /// "Fans also like" — artists similar to the given one.
    pub async fn get_artist_similar(
        &self,
        artist_id: i64,
        limit: i32,
        offset: i32,
    ) -> Result<TidalPaginatedResponse<TidalArtist>> {
        let url = format!(
            "{}/artists/{}/similar?countryCode={}&limit={}&offset={}",
            TIDAL_API_URL, artist_id, self.country_code, limit, offset
        );
        self.get_json(&url).await
    }

    // ─── Search ────────────────────────────────────────────

    pub async fn search_catalog(
        &self,
        query: &str,
        limit: i32,
        offset: i32,
    ) -> Result<TidalSearchCatalog> {
        let url = format!(
            "{}/search?query={}&countryCode={}&limit={}&offset={}&types=TRACKS,ALBUMS,ARTISTS,VIDEOS",
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
        let videos = payload
            .get("videos")
            .and_then(|videos| videos.get("items"))
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
            videos: videos
                .into_iter()
                .filter_map(Self::parse_search_video)
                .collect(),
        })
    }

    pub async fn search(&self, query: &str, limit: i32) -> Result<Vec<TidalSearchTrack>> {
        Ok(self.search_catalog(query, limit, 0).await?.tracks)
    }

    pub async fn search_videos(
        &self,
        query: &str,
        limit: i32,
        offset: i32,
    ) -> Result<Vec<TidalSearchVideo>> {
        let url = format!(
            "{}/search?query={}&countryCode={}&limit={}&offset={}&types=VIDEOS",
            TIDAL_API_URL,
            urlencoding::encode(query),
            self.country_code,
            limit,
            offset.max(0)
        );
        let payload: serde_json::Value = self.get_json(&url).await?;
        let videos = payload
            .get("videos")
            .and_then(|videos| videos.get("items"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();

        Ok(videos
            .into_iter()
            .filter_map(Self::parse_search_video)
            .collect())
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
    /// used by the TIDAL web client. HTTP errors (including 401) are propagated
    /// so the caller can handle token refresh. If TIDAL changes their response
    /// shape, `parse_my_mixes` returns an empty vec rather than erroring.
    ///
    /// Response shape: `{ rows: [{ modules: [{ pagedList: { items: [...] } }] }] }`
    /// where each item is a mix `{ id, title, subTitle, mixType, images: {...} }`.
    pub async fn get_my_mixes(&self) -> Result<Vec<TidalMix>> {
        let url = format!(
            "{}/pages/my_collection_my_mixes?countryCode={}&deviceType=BROWSER&locale=en_US",
            TIDAL_API_URL, self.country_code
        );
        let payload: serde_json::Value = self.get_json(&url).await?;
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

    /// Fetch the user's personal radio stations. They live as a single module
    /// inside `pages/for_you` titled "Radio stations for you" — the same
    /// module the web client renders as the "Personal radio stations" shelf.
    pub async fn get_my_radio_stations(&self) -> Result<Vec<TidalMix>> {
        let url = format!(
            "{}/pages/for_you?countryCode={}&deviceType=BROWSER&locale=en_US",
            TIDAL_API_URL, self.country_code
        );
        let payload: serde_json::Value = self.get_json(&url).await?;
        // Substring match — looser than the literal title so a Tidal rename
        // (e.g. "Personal radio stations") still matches. `for_you` only has
        // one radio-titled module so there's no ambiguity.
        Ok(Self::parse_module_by_title(&payload, "radio"))
    }

    /// Fetch all editorial modules from `pages/home` — what TIDAL's web client
    /// renders as the "discover" surface ("The Hits", "New Tracks", "New
    /// Albums", "Spotlighted Uploads", "From our editors", etc.). Each module
    /// gets its items parsed into a typed `TidalHomeItem` so the frontend can
    /// dispatch shelf rendering on `kind` without re-parsing TIDAL's wire shape.
    pub async fn get_home_modules(&self) -> Result<Vec<TidalHomeModule>> {
        // `limit` here is per-module — TIDAL caps each shelf's pagedList at 5
        // by default, so we ask for 12 to give the search surface room without
        // forcing the user to engage the rail scroll for every reveal.
        let url = format!(
            "{}/pages/home?countryCode={}&deviceType=BROWSER&locale=en_US&limit=12",
            TIDAL_API_URL, self.country_code
        );
        let payload: serde_json::Value = self.get_json(&url).await?;
        Ok(Self::parse_home_modules(&payload))
    }

    // Split out so a unit test can exercise the URL glue (separator selection,
    // country-code injection) without a live HTTP roundtrip. Callers pass the
    // path segment after `/v1/` (e.g. `"pages/charts"`, `"pages/mood/abc123"`,
    // or a path that already has its own query string).
    fn build_page_modules_url(
        api_url: &str,
        page_path: &str,
        country_code: &str,
        limit: u32,
    ) -> String {
        let separator = if page_path.contains('?') { '&' } else { '?' };
        format!(
            "{}/{}{}countryCode={}&deviceType=BROWSER&locale=en_US&limit={}",
            api_url, page_path, separator, country_code, limit
        )
    }

    /// Fetch editorial modules from any `/v1/pages/{page_path}` endpoint. Wraps
    /// the same parser as `get_home_modules` since TIDAL's page response shape
    /// (`rows[].modules[]`) is universal across home / charts / moods / genres /
    /// new-releases / mood/{id} / genre/{id}.
    pub async fn get_page_modules(&self, page_path: &str) -> Result<Vec<TidalHomeModule>> {
        let url = Self::build_page_modules_url(TIDAL_API_URL, page_path, &self.country_code, 12);
        let payload: serde_json::Value = self.get_json(&url).await?;
        Ok(Self::parse_home_modules(&payload))
    }

    /// Same fetch as `get_page_modules` but returns the unparsed upstream
    /// payload. Used by the `?debug=raw` debug query on the page route to
    /// expose TIDAL's module-type vocabulary while we firm up which slugs and
    /// shapes we need to handle.
    pub async fn get_page_raw(&self, page_path: &str) -> Result<serde_json::Value> {
        let url = Self::build_page_modules_url(TIDAL_API_URL, page_path, &self.country_code, 12);
        self.get_json(&url).await
    }

    fn parse_home_modules(payload: &serde_json::Value) -> Vec<TidalHomeModule> {
        let mut out = Vec::new();
        let Some(rows) = payload.get("rows").and_then(serde_json::Value::as_array) else {
            return out;
        };
        for row in rows {
            let Some(modules) = row.get("modules").and_then(serde_json::Value::as_array) else {
                continue;
            };
            for module in modules {
                let title = module
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if title.is_empty() {
                    continue;
                }
                let id = module
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let kind = module
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let items_arr = module
                    .get("pagedList")
                    .and_then(|p| p.get("items"))
                    .and_then(serde_json::Value::as_array)
                    .or_else(|| module.get("items").and_then(serde_json::Value::as_array));
                let Some(items_arr) = items_arr else { continue };
                let more_path = module
                    .get("pagedList")
                    .and_then(|p| p.get("dataApiPath"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);

                // Items inside `pages/home` modules are flat objects (no
                // `{type, item}` wrapper) and don't carry a per-item type
                // field. The module's own `kind` tells us how to parse them:
                // TRACK_LIST / ALBUM_LIST / PLAYLIST_LIST. MIXED_TYPES_LIST
                // (rare) falls back to per-item shape detection.
                let items: Vec<TidalHomeItem> = items_arr
                    .iter()
                    .filter_map(|item| Self::parse_home_item(item, &kind))
                    .collect();
                if items.is_empty() {
                    continue;
                }
                tracing::debug!(
                    "TIDAL home module '{}' (kind={}): parsed {}/{} items, more_path={:?}",
                    title,
                    kind,
                    items.len(),
                    items_arr.len(),
                    more_path
                );
                out.push(TidalHomeModule {
                    id,
                    title,
                    kind,
                    more_path,
                    items,
                });
            }
        }
        out
    }

    /// Fetch the full item set for one home module via its
    /// `pagedList.dataApiPath`. The home `pages/home` response only ships a
    /// 5-item preview for TRACK_LIST modules, so the per-module "View all"
    /// detail route follows `dataApiPath` to get the rest.
    pub async fn get_module_items_via_path(
        &self,
        more_path: &str,
        module_kind: &str,
        limit: u32,
    ) -> Result<Vec<TidalHomeItem>> {
        let separator = if more_path.contains('?') { '&' } else { '?' };
        let url = format!(
            "{}/{}{}countryCode={}&deviceType=BROWSER&locale=en_US&limit={}",
            TIDAL_API_URL, more_path, separator, self.country_code, limit
        );
        let payload: serde_json::Value = self.get_json(&url).await?;
        // "show more" endpoints return either a top-level pagedList or a
        // wrapped row/module shape — unwrap whichever we get.
        let items_arr: Vec<serde_json::Value> = payload
            .get("items")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .or_else(|| {
                payload
                    .get("pagedList")
                    .and_then(|p| p.get("items"))
                    .and_then(serde_json::Value::as_array)
                    .cloned()
            })
            .or_else(|| {
                let rows = payload.get("rows").and_then(serde_json::Value::as_array)?;
                let mut all = Vec::new();
                for row in rows {
                    if let Some(modules) = row.get("modules").and_then(serde_json::Value::as_array)
                    {
                        for m in modules {
                            if let Some(items) = m
                                .get("pagedList")
                                .and_then(|p| p.get("items"))
                                .and_then(serde_json::Value::as_array)
                            {
                                all.extend(items.iter().cloned());
                            }
                        }
                    }
                }
                Some(all)
            })
            .unwrap_or_default();
        Ok(items_arr
            .iter()
            .filter_map(|item| Self::parse_home_item(item, module_kind))
            .collect())
    }

    /// Decode one item from a `pages/home` module. Items are flat objects
    /// (no wrapper/type field), so we dispatch on the module's `kind`. For
    /// `MIXED_TYPES_LIST` we sniff the item shape — `uuid` => playlist,
    /// `cover` => album, otherwise track.
    fn parse_home_item(item: &serde_json::Value, module_kind: &str) -> Option<TidalHomeItem> {
        let resolved_kind = match module_kind {
            "TRACK_LIST" => "track",
            "ALBUM_LIST" => "album",
            "PLAYLIST_LIST" => "playlist",
            _ => {
                let obj = item.as_object()?;
                if obj.contains_key("uuid") {
                    "playlist"
                } else if obj.contains_key("cover") {
                    "album"
                } else {
                    "track"
                }
            }
        };
        match resolved_kind {
            "track" => {
                let t = Self::parse_search_track(item.clone())?;
                Some(TidalHomeItem {
                    kind: "track".into(),
                    id: t.id.to_string(),
                    title: t.title,
                    artist_name: t.artist_name,
                    artwork_url: t.artwork_url,
                    duration: Some(t.duration),
                    artist_id: t.artist_id,
                    album_id: t.album_id,
                    album_title: t.album_title,
                    creator_name: None,
                })
            }
            "album" => {
                let a = Self::parse_search_album(item.clone())?;
                let artist_id = item
                    .get("artist")
                    .and_then(|v| v.get("id"))
                    .and_then(serde_json::Value::as_i64)
                    .or_else(|| {
                        item.get("artists")
                            .and_then(serde_json::Value::as_array)
                            .and_then(|a| a.first())
                            .and_then(|v| v.get("id"))
                            .and_then(serde_json::Value::as_i64)
                    });
                Some(TidalHomeItem {
                    kind: "album".into(),
                    id: a.id.to_string(),
                    title: a.title,
                    artist_name: a.artist_name,
                    artwork_url: a.artwork_url,
                    duration: None,
                    artist_id,
                    album_id: Some(a.id),
                    album_title: None,
                    creator_name: None,
                })
            }
            "playlist" => Self::parse_home_playlist(item),
            _ => None,
        }
    }

    fn parse_home_playlist(value: &serde_json::Value) -> Option<TidalHomeItem> {
        let obj = value.as_object()?;
        let uuid = obj
            .get("uuid")
            .and_then(serde_json::Value::as_str)?
            .to_string();
        let title = obj
            .get("title")
            .and_then(serde_json::Value::as_str)?
            .to_string();
        // TIDAL ships playlist authors as a `creators[]` array; fall back to
        // legacy `creator.name` for older payload shapes.
        let creator_name = obj
            .get("creators")
            .and_then(serde_json::Value::as_array)
            .and_then(|a| a.first())
            .and_then(|c| c.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                obj.get("creator")
                    .and_then(|c| c.get("name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
        // Prefer `squareImage` (uuid keyed for the square collage), falling
        // back to `image` (regular cover). Both feed the standard artwork
        // builder which appends size + extension.
        let artwork_url = obj
            .get("squareImage")
            .and_then(serde_json::Value::as_str)
            .or_else(|| obj.get("image").and_then(serde_json::Value::as_str))
            .map(|p| Self::artwork_url(p, 640));
        Some(TidalHomeItem {
            kind: "playlist".into(),
            id: uuid,
            title,
            artist_name: None,
            artwork_url,
            duration: None,
            artist_id: None,
            album_id: None,
            album_title: None,
            creator_name,
        })
    }

    /// Walk `rows[].modules[]` and return items from the first module whose
    /// title contains `needle` (case-insensitive). Items are parsed as mixes.
    fn parse_module_by_title(payload: &serde_json::Value, needle: &str) -> Vec<TidalMix> {
        let needle = needle.to_ascii_lowercase();
        let Some(rows) = payload.get("rows").and_then(serde_json::Value::as_array) else {
            return Vec::new();
        };
        for row in rows {
            let Some(modules) = row.get("modules").and_then(serde_json::Value::as_array) else {
                continue;
            };
            for module in modules {
                let title = module
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if !title.contains(&needle) {
                    continue;
                }
                let items = module
                    .get("pagedList")
                    .and_then(|p| p.get("items"))
                    .and_then(serde_json::Value::as_array)
                    .or_else(|| module.get("items").and_then(serde_json::Value::as_array));
                let Some(items) = items else { continue };
                return items.iter().filter_map(Self::parse_mix_item).collect();
            }
        }
        Vec::new()
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

    pub async fn get_video_mix_items(&self, mix_id: &str) -> Result<Vec<TidalSearchVideo>> {
        let url = format!(
            "{}/mixes/{}/items?countryCode={}&limit=100&includeTypes=MusicVideo",
            TIDAL_API_URL, mix_id, self.country_code
        );
        let payload: serde_json::Value = self.get_json(&url).await?;
        Ok(Self::parse_mix_video_items(&payload))
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

    fn parse_mix_video_items(payload: &serde_json::Value) -> Vec<TidalSearchVideo> {
        let Some(items) = payload.get("items").and_then(serde_json::Value::as_array) else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|wrapper| {
                let item = wrapper.get("item").unwrap_or(wrapper);
                let video_type = wrapper
                    .get("type")
                    .or_else(|| item.get("type"))
                    .or_else(|| item.get("contentType"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if !video_type.is_empty()
                    && !video_type.contains("video")
                    && !video_type.contains("musicvideo")
                {
                    return None;
                }
                Self::parse_search_video(item.clone())
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
        if out.is_empty()
            && let Some(items) = payload.get("items").and_then(serde_json::Value::as_array)
        {
            for item in items {
                if let Some(mix) = Self::parse_mix_item(item) {
                    out.push(mix);
                }
            }
        }

        // Shape 3 (some regions): top-level mixes[]
        if out.is_empty()
            && let Some(items) = payload.get("mixes").and_then(serde_json::Value::as_array)
        {
            for item in items {
                if let Some(mix) = Self::parse_mix_item(item) {
                    out.push(mix);
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
        let is_video_mix =
            Self::detect_video_mix(&title, sub_title.as_deref(), mix_type.as_deref());
        Some(TidalMix {
            id,
            title,
            sub_title,
            image_url,
            mix_type,
            is_video_mix,
        })
    }

    fn detect_video_mix(title: &str, sub_title: Option<&str>, mix_type: Option<&str>) -> bool {
        fn norm(value: &str) -> String {
            value
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() {
                        ch.to_ascii_lowercase()
                    } else {
                        ' '
                    }
                })
                .collect::<String>()
        }

        if mix_type.is_some_and(|value| {
            let value = norm(value);
            value.contains("video") || value.contains("music video") || value.contains("musicvideo")
        }) {
            return true;
        }

        [Some(title), sub_title]
            .into_iter()
            .flatten()
            .map(norm)
            .any(|value| value.contains("video mix") || value.contains("music video"))
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
                if let Some(v) = obj.get(key)
                    && let Some(u) = direct_url_or_image_id(v)
                {
                    return Some(u);
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

    fn pick_video_artwork(object: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
        for key in ["imageId", "image_id", "cover", "squareImage", "image"] {
            if let Some(value) = object.get(key) {
                if let Some(url) = direct_url_or_image_id(value) {
                    return Some(url);
                }
                if let Some(raw) = value.as_str().filter(|s| !s.is_empty()) {
                    return Some(
                        if raw.starts_with("http://") || raw.starts_with("https://") {
                            raw.to_string()
                        } else {
                            Self::artwork_url(raw, 640)
                        },
                    );
                }
            }
        }

        object
            .get("album")
            .and_then(serde_json::Value::as_object)
            .and_then(|album| {
                album
                    .get("cover")
                    .or_else(|| album.get("imageId"))
                    .or_else(|| album.get("image"))
                    .and_then(direct_url_or_image_id)
            })
            .or_else(|| Self::pick_mix_image(object.get("images")))
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

    fn parse_search_video(value: serde_json::Value) -> Option<TidalSearchVideo> {
        let object = value.as_object()?;
        let id = object
            .get("id")
            .and_then(|id| id.as_i64().or_else(|| id.as_str()?.parse().ok()))?;
        let title = object.get("title")?.as_str()?.to_string();
        let duration = object
            .get("duration")
            .and_then(serde_json::Value::as_i64)
            .or_else(|| {
                object
                    .get("durationMs")
                    .or_else(|| object.get("duration_ms"))
                    .and_then(serde_json::Value::as_i64)
                    .map(|ms| ms / 1000)
            });
        let primary_artist = object.get("artist").or_else(|| {
            object
                .get("artists")
                .and_then(serde_json::Value::as_array)
                .and_then(|artists| artists.first())
        });
        let artist_id = primary_artist
            .and_then(|artist| artist.get("id"))
            .and_then(serde_json::Value::as_i64);
        let artist_name = primary_artist
            .and_then(|artist| artist.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                object
                    .get("artistName")
                    .or_else(|| object.get("artist_name"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
        let album_id = object
            .get("album")
            .and_then(|album| album.get("id"))
            .and_then(serde_json::Value::as_i64)
            .or_else(|| {
                object
                    .get("albumId")
                    .or_else(|| object.get("album_id"))
                    .and_then(serde_json::Value::as_i64)
            });
        let artwork_url = Self::pick_video_artwork(object);
        let quality = object
            .get("quality")
            .or_else(|| object.get("videoQuality"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let explicit = object
            .get("explicit")
            .or_else(|| object.get("explicitContent"))
            .and_then(serde_json::Value::as_bool);
        let r#type = object
            .get("type")
            .or_else(|| object.get("contentType"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("video")
            .to_string();
        let extra = object.clone().into_iter().collect();

        Some(TidalSearchVideo {
            id,
            title,
            duration,
            artist_id,
            artist_name,
            album_id,
            artwork_url,
            quality,
            explicit,
            r#type,
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
    pub is_video_mix: bool,
}

/// One item rendered inside a TIDAL home-page module. Flat shape (rather
/// than a tagged enum) so the frontend can dispatch on `kind` without
/// dealing with serde-tagged unions in TS. Per-kind fields are optional —
/// e.g. `duration` is only set for tracks, `creator_name` only for playlists.
#[derive(Debug, Clone, Serialize)]
pub struct TidalHomeItem {
    /// `"track" | "album" | "playlist"` — the only kinds we surface today.
    pub kind: String,
    /// Stringified for tracks/albums (numeric id), uuid for playlists.
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creator_name: Option<String>,
}

/// One module on a TIDAL `pages/*` response. Editorial home shelves like
/// "The Hits", "New Albums", "Spotlighted Uploads" are each a module.
#[derive(Debug, Clone, Serialize)]
pub struct TidalHomeModule {
    pub id: String,
    pub title: String,
    /// TIDAL's module type — `TRACK_LIST`, `ALBUM_LIST`, `PLAYLIST_LIST`,
    /// `MIXED_TYPES_LIST`, etc. Pass-through; the frontend only inspects
    /// per-item `kind`, but `kind` here lets ops debug.
    pub kind: String,
    /// TIDAL's `pagedList.dataApiPath` — the URL the web client follows when
    /// the user clicks "View all". We resolve `more_path` server-side in the
    /// per-module detail handler so the frontend never needs to know the
    /// upstream URL shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more_path: Option<String>,
    pub items: Vec<TidalHomeItem>,
}

/// Pull a renderable URL out of the various shapes TIDAL returns for an
/// `images` entry: direct `url`, or an `imageId` we can route through the
/// standard `resources.tidal.com` artwork builder.
fn direct_url_or_image_id(v: &serde_json::Value) -> Option<String> {
    if let Some(raw) = v.as_str().filter(|s| !s.is_empty()) {
        return Some(
            if raw.starts_with("http://") || raw.starts_with("https://") {
                raw.to_string()
            } else {
                TidalClient::artwork_url(raw, 640)
            },
        );
    }
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
    use std::sync::{Arc, Mutex};
    use tracing::{
        Event, Id, Level, Metadata, Subscriber,
        span::{Attributes, Record},
        subscriber::Interest,
    };

    #[derive(Clone, Default)]
    struct RecordingSubscriber {
        levels: Arc<Mutex<Vec<Level>>>,
    }

    impl Subscriber for RecordingSubscriber {
        fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
            Interest::always()
        }

        fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
            Some(tracing::metadata::LevelFilter::TRACE)
        }

        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            self.levels.lock().unwrap().push(*event.metadata().level());
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    /// Round-trip the documented `pages/my_collection_my_mixes` shape through
    /// the parser. Asserts: every mix has non-empty id/title/image_url, the
    /// mix_type passes through, and rows/modules nesting is walked.
    #[test]
    fn favorite_urls_request_newest_first_ordering() {
        let client = TidalClient::new("token".into(), "US".into());

        let tracks = client.favorite_tracks_url("user-1", 100, 0);
        let albums = client.favorite_albums_url("user-1", 100, 20);
        let artists = client.favorite_artists_url("user-1", 50, 10);

        for url in [tracks, albums, artists] {
            assert!(
                url.contains("order=DATE"),
                "favorite URL must request date ordering: {url}"
            );
            assert!(
                url.contains("orderDirection=DESC"),
                "favorite URL must request newest-first ordering: {url}"
            );
        }
    }

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
                                    },
                                    {
                                        "id": "0789feed",
                                        "title": "My Video Mix 1",
                                        "subTitle": "Music videos picked for you",
                                        "mixType": "VIDEO_MIX",
                                        "images": [
                                            { "url": "https://img.tidal.com/video.jpg" }
                                        ]
                                    }
                                ]
                            }
                        }
                    ]
                }
            ]
        });
        let mixes = TidalClient::parse_my_mixes(&payload);
        assert_eq!(mixes.len(), 3, "expected 3 mixes parsed from fixture");
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
        assert!(!mixes[0].is_video_mix);
        assert!(!mixes[1].is_video_mix);
        assert!(mixes[2].is_video_mix);
    }

    #[test]
    fn detects_video_mix_from_mix_metadata() {
        assert!(TidalClient::detect_video_mix(
            "My Mix 3",
            None,
            Some("VIDEO_MIX")
        ));
        assert!(TidalClient::detect_video_mix(
            "My Video Mix 2",
            Some("Your videos"),
            None
        ));
        assert!(TidalClient::detect_video_mix(
            "Fresh picks",
            Some("Music videos from favorites"),
            None
        ));
        assert!(!TidalClient::detect_video_mix(
            "My Mix 2",
            Some("Video Age, The Strokes"),
            Some("PERSONAL")
        ));
        assert!(!TidalClient::detect_video_mix(
            "Daily Discovery",
            Some("Updated daily"),
            Some("DAILY_DISCOVERY")
        ));
    }

    /// An empty / malformed payload must yield an empty list, not panic.
    #[test]
    fn parse_my_mixes_handles_missing_rows() {
        assert!(TidalClient::parse_my_mixes(&json!({})).is_empty());
        assert!(TidalClient::parse_my_mixes(&json!({"rows": []})).is_empty());
        assert!(TidalClient::parse_my_mixes(&json!({"rows": [{"modules": []}]})).is_empty());
    }

    #[test]
    fn parse_home_modules_logs_success_at_debug_level() {
        let payload = json!({
            "rows": [
                {
                    "modules": [
                        {
                            "id": "new-tracks",
                            "title": "New Tracks",
                            "type": "TRACK_LIST",
                            "pagedList": {
                                "dataApiPath": "pages/data/new-tracks",
                                "items": [
                                    {
                                        "id": 10,
                                        "title": "Signal",
                                        "duration": 180,
                                        "artist": { "id": 20, "name": "NOOR" },
                                        "album": { "id": 30, "title": "Wave", "cover": "aaa-bbb-ccc" }
                                    }
                                ]
                            }
                        }
                    ]
                }
            ]
        });
        let subscriber = RecordingSubscriber::default();
        let levels = subscriber.levels.clone();

        let modules =
            tracing::dispatcher::with_default(&tracing::Dispatch::new(subscriber), || {
                TidalClient::parse_home_modules(&payload)
            });

        assert_eq!(modules.len(), 1);
        let levels = levels.lock().unwrap().clone();
        assert!(
            !levels.contains(&Level::WARN),
            "successful home module parse must not emit warn"
        );
        assert!(
            levels.contains(&Level::DEBUG),
            "successful home module parse should remain visible at debug"
        );
    }

    #[test]
    fn parse_search_video_extracts_renderable_fields() {
        let video = TidalClient::parse_search_video(json!({
            "id": 123,
            "title": "A Good Video",
            "duration": 245,
            "artist": { "id": 55, "name": "NOOR" },
            "album": { "id": 66, "cover": "abc-def-ghi" },
            "videoQuality": "HIGH",
            "explicit": true,
            "type": "Music Video"
        }))
        .expect("video should parse");

        assert_eq!(video.id, 123);
        assert_eq!(video.duration, Some(245));
        assert_eq!(video.artist_id, Some(55));
        assert_eq!(video.artist_name.as_deref(), Some("NOOR"));
        assert_eq!(video.album_id, Some(66));
        assert_eq!(video.quality.as_deref(), Some("HIGH"));
        assert_eq!(video.explicit, Some(true));
        assert!(
            video.artwork_url.as_deref().is_some_and(
                |url| url.starts_with("https://resources.tidal.com/images/abc/def/ghi/")
            )
        );
    }

    #[test]
    fn parse_video_mix_items_filters_non_video_wrappers() {
        let payload = json!({
            "items": [
                {
                    "type": "MusicVideo",
                    "item": {
                        "id": 1,
                        "title": "Visual",
                        "duration": 180,
                        "artists": [{ "id": 9, "name": "Artist" }],
                        "imageId": "111-222-333"
                    }
                },
                {
                    "type": "TRACK",
                    "item": {
                        "id": 2,
                        "title": "Audio only",
                        "duration": 180
                    }
                }
            ]
        });

        let items = TidalClient::parse_mix_video_items(&payload);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, 1);
        assert_eq!(items[0].artist_name.as_deref(), Some("Artist"));
    }

    /// Regression: TIDAL's per-track payload ships `trackNumber` / `volumeNumber`
    /// in camelCase. Both fields had no #[serde(rename)] for a long time, so
    /// every TIDAL-imported track row stored NULL and the album sort fell
    /// back to alphabetical (e.g. Red Headed Stranger out of narrative order).
    #[test]
    fn tidal_track_deserializes_camel_case_numbers() {
        let payload = json!({
            "id": 12345,
            "title": "Time of the Preacher",
            "duration": 167,
            "trackNumber": 1,
            "volumeNumber": 1,
            "isrc": "USAB10000001",
            "artist": { "id": 1, "name": "Willie Nelson" }
        });
        let track: TidalTrack = serde_json::from_value(payload).unwrap();
        assert_eq!(track.track_number, Some(1));
        assert_eq!(track.volume_number, Some(1));
    }

    #[test]
    fn build_page_modules_url_uses_query_separator_correctly() {
        let plain = TidalClient::build_page_modules_url(
            "https://api.tidal.com/v1",
            "pages/charts",
            "US",
            12,
        );
        assert_eq!(
            plain,
            "https://api.tidal.com/v1/pages/charts?countryCode=US&deviceType=BROWSER&locale=en_US&limit=12",
        );

        let already_queried = TidalClient::build_page_modules_url(
            "https://api.tidal.com/v1",
            "pages/mood/abc?foo=bar",
            "GB",
            50,
        );
        assert_eq!(
            already_queried,
            "https://api.tidal.com/v1/pages/mood/abc?foo=bar&countryCode=GB&deviceType=BROWSER&locale=en_US&limit=50",
        );
    }
}
