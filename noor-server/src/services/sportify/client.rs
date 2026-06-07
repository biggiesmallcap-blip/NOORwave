//! Thin reqwest wrapper over the 8 Sportify endpoints.
//!
//! Sportify exposes:
//!   GET /api/health
//!   GET /api/token              (we don't expose; Sportify manages it)
//!   GET /api/search?q=&type=&limit=&offset=
//!   GET /api/track/:id
//!   GET /api/album/:id
//!   GET /api/playlist/:id
//!   GET /api/artist/:id
//!   GET /api/artist/:id/top-tracks
//!
//! No auth required. Standard envelope: `{ success, error?, ...payload }`.

use anyhow::{Context, Result, anyhow};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::models::{
    SportifyAlbum, SportifyArtist, SportifyPlaylist, SportifySearchResults, SportifyTrack,
};

#[derive(Debug, Clone)]
pub struct SportifyClientConfig {
    pub base_url: String,
    pub fallback_base_urls: Vec<String>,
    pub user_agent: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_flat_search_results_array() {
        let value = json!({
            "success": true,
            "results": [{ "id": "spotify-playlist-1", "name": "Lofi" }]
        });

        let items = extract_search_items(&value, SportifySearchKind::Playlist);
        assert_eq!(items.as_array().unwrap().len(), 1);
        assert_eq!(items[0]["id"], "spotify-playlist-1");
    }

    #[test]
    fn extracts_nested_search_results_items() {
        let value = json!({
            "success": true,
            "results": {
                "items": [{ "id": "spotify-playlist-2", "name": "Vietnamese Classics" }]
            }
        });

        let items = extract_search_items(&value, SportifySearchKind::Playlist);
        assert_eq!(items.as_array().unwrap().len(), 1);
        assert_eq!(items[0]["id"], "spotify-playlist-2");
    }

    #[test]
    fn extracts_typed_playlist_bucket() {
        let value = json!({
            "success": true,
            "playlists": {
                "items": [{ "id": "spotify-playlist-3", "name": "Deep Focus" }]
            }
        });

        let items = extract_search_items(&value, SportifySearchKind::Playlist);
        assert_eq!(items.as_array().unwrap().len(), 1);
        assert_eq!(items[0]["id"], "spotify-playlist-3");
    }

    #[test]
    fn client_config_collects_fallback_base_urls() {
        let client = SportifyClient::new(SportifyClientConfig {
            base_url: "https://primary.example/".to_string(),
            fallback_base_urls: vec![
                "https://fallback.example/".to_string(),
                "https://primary.example".to_string(),
            ],
            user_agent: "noor-test".to_string(),
        })
        .expect("client");

        assert_eq!(
            client.base_urls,
            vec![
                "https://primary.example".to_string(),
                "https://fallback.example".to_string()
            ]
        );
    }

    #[test]
    fn resource_paths_encode_spotify_id_as_single_segment() {
        assert_eq!(
            sportify_resource_path("track", "abc/def?x=1"),
            "/api/track/abc%2Fdef%3Fx%3D1"
        );
        assert_eq!(
            sportify_artist_top_tracks_path("artist/id#frag"),
            "/api/artist/artist%2Fid%23frag/top-tracks"
        );
    }

    #[test]
    fn resource_paths_keep_plain_spotify_id_shape() {
        assert_eq!(
            sportify_resource_path("playlist", "37i9dQZF1DXcBWIGoYBM5M"),
            "/api/playlist/37i9dQZF1DXcBWIGoYBM5M"
        );
    }

    #[test]
    fn truncate_for_log_is_utf8_boundary_safe() {
        let raw = "é".repeat(200);
        let truncated = truncate_for_log(&raw);

        assert!(truncated.ends_with("..."));
        assert!(truncated.len() <= 259);
    }
}

impl Default for SportifyClientConfig {
    fn default() -> Self {
        Self {
            base_url: "https://sportify.xcasper.space".to_string(),
            fallback_base_urls: Vec::new(),
            user_agent: "noor-server/sportify".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SportifySearchKind {
    Track,
    Album,
    Artist,
    Playlist,
}

impl SportifySearchKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Track => "track",
            Self::Album => "album",
            Self::Artist => "artist",
            Self::Playlist => "playlist",
        }
    }

    fn plural_str(&self) -> &'static str {
        match self {
            Self::Track => "tracks",
            Self::Album => "albums",
            Self::Artist => "artists",
            Self::Playlist => "playlists",
        }
    }
}

#[derive(Clone)]
pub struct SportifyClient {
    http: reqwest::Client,
    base_urls: Vec<String>,
}

impl SportifyClient {
    pub fn new(config: SportifyClientConfig) -> Result<Self> {
        // reqwest has no default timeout. The Sportify proxy is a third-party
        // service and several UI surfaces make best-effort calls while the user
        // is typing, so a hung upstream would otherwise keep work alive too long.
        // Bound both the connect and total time; normal responses are well
        // under a second, and best-effort callers treat a timeout as "no data".
        let http = reqwest::Client::builder()
            .user_agent(config.user_agent)
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .context("build sportify reqwest client")?;
        let mut base_urls = Vec::new();
        for raw in std::iter::once(config.base_url).chain(config.fallback_base_urls) {
            let normalized = raw.trim().trim_end_matches('/').to_string();
            if !normalized.is_empty() && !base_urls.iter().any(|u| u == &normalized) {
                base_urls.push(normalized);
            }
        }
        if base_urls.is_empty() {
            return Err(anyhow!("no sportify base URL configured"));
        }
        Ok(Self { http, base_urls })
    }

    /// Issue a GET, validate the standard envelope, and return the raw body
    /// JSON (envelope fields included). Callers extract the resource field.
    async fn fetch_raw(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        let mut errors = Vec::new();
        for base_url in &self.base_urls {
            match self.fetch_raw_from_base(base_url, path, query).await {
                Ok(value) => return Ok(value),
                Err(e) => {
                    tracing::debug!("sportify {} via {} failed: {}", path, base_url, e);
                    errors.push(format!("{}: {}", base_url, e));
                }
            }
        }

        Err(anyhow!(
            "all sportify proxies failed for {}: {}",
            path,
            errors.join("; ")
        ))
    }

    async fn fetch_raw_from_base(
        &self,
        base_url: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Value> {
        let url = format!("{}{}", base_url, path);
        let resp = self
            .http
            .get(&url)
            .query(query)
            .send()
            .await
            .with_context(|| format!("sportify request failed: {}", path))?;

        let status = resp.status();
        let body = resp.text().await.context("read sportify body")?;
        if !status.is_success() {
            return Err(anyhow!(
                "sportify {} returned HTTP {}: {}",
                path,
                status,
                truncate_for_log(&body)
            ));
        }

        let value: Value = serde_json::from_str(&body)
            .with_context(|| format!("parse sportify json: {}", path))?;

        let success = value
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !success {
            let err = value
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(anyhow!("sportify {} unsuccessful: {}", path, err));
        }

        Ok(value)
    }

    /// Resource endpoints wrap the entity in a field named after the
    /// resource (e.g. `{ "success": true, "track": {...} }`). Try that field
    /// first; fall back to root for older response shapes.
    fn extract_field<T: DeserializeOwned>(value: Value, field: &str) -> Result<T> {
        if let Some(inner) = value.get(field).cloned() {
            serde_json::from_value(inner)
                .with_context(|| format!("parse sportify {} payload", field))
        } else {
            serde_json::from_value(value)
                .with_context(|| format!("parse sportify {} (root)", field))
        }
    }

    pub async fn search(
        &self,
        query: &str,
        kind: SportifySearchKind,
        limit: u32,
        offset: u32,
    ) -> Result<SportifySearchResults> {
        let limit = limit.clamp(1, 50);
        let query_params = [
            ("q", query.to_string()),
            ("type", kind.as_str().to_string()),
            ("limit", limit.to_string()),
            ("offset", offset.to_string()),
        ];
        let mut errors = Vec::new();
        for (idx, base_url) in self.base_urls.iter().enumerate() {
            match self
                .fetch_raw_from_base(base_url, "/api/search", &query_params)
                .await
            {
                Ok(value) => {
                    let out = search_results_from_value(&value, kind);
                    if matches!(kind, SportifySearchKind::Playlist)
                        && out.playlists.is_empty()
                        && idx + 1 < self.base_urls.len()
                    {
                        tracing::debug!(
                            "sportify playlist search via {} returned no playlist rows",
                            base_url
                        );
                        continue;
                    }
                    return Ok(out);
                }
                Err(e) => {
                    tracing::debug!("sportify /api/search via {} failed: {}", base_url, e);
                    errors.push(format!("{}: {}", base_url, e));
                }
            }
        }

        Err(anyhow!(
            "all sportify proxies failed for /api/search: {}",
            errors.join("; ")
        ))
    }

    pub async fn track(&self, spotify_id: &str) -> Result<SportifyTrack> {
        let value = self
            .fetch_raw(&sportify_resource_path("track", spotify_id), &[])
            .await?;
        Self::extract_field(value, "track")
    }

    pub async fn album(&self, spotify_id: &str) -> Result<SportifyAlbum> {
        let value = self
            .fetch_raw(&sportify_resource_path("album", spotify_id), &[])
            .await?;
        Self::extract_field(value, "album")
    }

    pub async fn playlist(&self, spotify_id: &str) -> Result<SportifyPlaylist> {
        let value = self
            .fetch_raw(&sportify_resource_path("playlist", spotify_id), &[])
            .await?;
        Self::extract_field(value, "playlist")
    }

    pub async fn artist(&self, spotify_id: &str) -> Result<SportifyArtist> {
        let value = self
            .fetch_raw(&sportify_resource_path("artist", spotify_id), &[])
            .await?;
        Self::extract_field(value, "artist")
    }

    pub async fn artist_top_tracks(&self, spotify_id: &str) -> Result<Vec<SportifyTrack>> {
        let value = self
            .fetch_raw(&sportify_artist_top_tracks_path(spotify_id), &[])
            .await?;
        if let Some(inner) = value.get("tracks").cloned() {
            return serde_json::from_value(inner).context("parse sportify top-tracks `tracks`");
        }
        if let Some(inner) = value.get("items").cloned() {
            return serde_json::from_value(inner).context("parse sportify top-tracks `items`");
        }
        Ok(Vec::new())
    }
}

fn extract_search_items(value: &Value, kind: SportifySearchKind) -> Value {
    fn unwrap_items(value: &Value, kind: SportifySearchKind) -> Option<Value> {
        if value.is_array() {
            return Some(value.clone());
        }
        value
            .get("items")
            .cloned()
            .or_else(|| value.get(kind.as_str()).cloned())
            .or_else(|| value.get(kind.plural_str()).cloned())
            .and_then(|v| unwrap_items(&v, kind).or(Some(v)))
    }

    value
        .get("results")
        .and_then(|v| unwrap_items(v, kind))
        .or_else(|| value.get(kind.as_str()).and_then(|v| unwrap_items(v, kind)))
        .or_else(|| {
            value
                .get(kind.plural_str())
                .and_then(|v| unwrap_items(v, kind))
        })
        .unwrap_or_else(|| Value::Array(Vec::new()))
}

fn search_results_from_value(value: &Value, kind: SportifySearchKind) -> SportifySearchResults {
    // Sportify search usually returns `{ results: [...] }`, but has also
    // shipped nested `{ results: { items: [...] } }` and typed buckets.
    // Normalize all of those into the requested result bucket.
    let mut out = SportifySearchResults::default();
    let array = extract_search_items(value, kind);

    match kind {
        SportifySearchKind::Track => {
            out.tracks = serde_json::from_value(array).unwrap_or_default();
        }
        SportifySearchKind::Album => {
            out.albums = serde_json::from_value(array).unwrap_or_default();
        }
        SportifySearchKind::Artist => {
            out.artists = serde_json::from_value(array).unwrap_or_default();
        }
        SportifySearchKind::Playlist => {
            out.playlists = serde_json::from_value(array).unwrap_or_default();
        }
    }
    out
}

fn sportify_resource_path(resource: &str, spotify_id: &str) -> String {
    format!("/api/{}/{}", resource, urlencoding::encode(spotify_id))
}

fn sportify_artist_top_tracks_path(spotify_id: &str) -> String {
    format!("/api/artist/{}/top-tracks", urlencoding::encode(spotify_id))
}

fn truncate_for_log(s: &str) -> String {
    if s.len() > 256 {
        let end = s
            .char_indices()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .take_while(|end| *end <= 256)
            .last()
            .unwrap_or(0);
        format!("{}...", &s[..end])
    } else {
        s.to_string()
    }
}
