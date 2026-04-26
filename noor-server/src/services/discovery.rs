use crate::db::models::DiscoveryProviderCapability;
use crate::genre::builder::embedded_builder;
use crate::services::tidal::{
    client::{TidalClient, TidalSearchArtist, TidalSearchCatalog, TidalSearchTrack},
    mutations as tidal_mutations,
};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct DiscoveryCandidateTrack {
    pub provider: String,
    pub provider_track_id: String,
    pub tidal_track_id: Option<i64>,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub audio_quality: Option<String>,
    pub raw_genre_hints: Vec<String>,
    pub lastfm_tags: Vec<String>,
    pub discogs_genres: Vec<String>,
    pub discogs_styles: Vec<String>,
    pub discogs_label: Option<String>,
    pub discogs_year: Option<i32>,
    pub discogs_confidence: Option<f64>,
    pub is_playable: bool,
    pub metadata_tokens: Vec<String>,
    pub seed_kind: String,
    pub seed_strength: i32,
}

#[derive(Debug, Clone)]
pub struct DiscoveryCandidateSeed {
    pub provider_track_id: String,
    pub title: String,
    pub artist_name: Option<String>,
    pub album_title: Option<String>,
    pub normalized_genres: Vec<String>,
}

#[async_trait]
pub trait DiscoveryProvider: Send + Sync {
    fn capabilities(&self) -> DiscoveryProviderCapability;

    async fn search_tracks(
        &self,
        queries: &[String],
        limit_per_query: usize,
    ) -> Result<Vec<DiscoveryCandidateTrack>>;

    async fn connected_tracks(
        &self,
        seed: &DiscoveryCandidateSeed,
        queries: &[String],
        limit_per_query: usize,
    ) -> Result<Vec<DiscoveryCandidateTrack>>;

    async fn save_track(&self, provider_track_id: &str) -> Result<()>;
}

#[derive(Clone)]
pub struct TidalDiscoveryProvider {
    client: TidalClient,
    http: reqwest::Client,
    access_token: String,
    user_id: String,
    country_code: String,
}

impl TidalDiscoveryProvider {
    pub fn new(
        access_token: String,
        user_id: String,
        country_code: String,
        http: reqwest::Client,
    ) -> Self {
        Self {
            client: TidalClient::new(access_token.clone(), country_code.clone()),
            http,
            access_token,
            user_id,
            country_code,
        }
    }

    fn map_track(
        track: TidalSearchTrack,
        seed_kind: &str,
        seed_strength: i32,
    ) -> DiscoveryCandidateTrack {
        let raw_genre_hints = collect_tidal_genre_hints(&track);
        let metadata_tokens = collect_metadata_tokens(&track, &raw_genre_hints);

        DiscoveryCandidateTrack {
            provider: "tidal".to_string(),
            provider_track_id: track.id.to_string(),
            tidal_track_id: Some(track.id),
            title: track.title,
            artist_name: track.artist_name,
            album_title: track.album_title,
            artwork_url: track.artwork_url,
            duration_ms: Some(track.duration.saturating_mul(1000)),
            audio_quality: track.audio_quality,
            raw_genre_hints,
            lastfm_tags: Vec::new(),
            discogs_genres: Vec::new(),
            discogs_styles: Vec::new(),
            discogs_label: None,
            discogs_year: None,
            discogs_confidence: None,
            is_playable: track.stream_ready.unwrap_or(true),
            metadata_tokens,
            seed_kind: seed_kind.to_string(),
            seed_strength,
        }
    }

    fn artist_followups(search_query: &str, artists: &[TidalSearchArtist]) -> Vec<(String, i32)> {
        let query_tail = search_query.trim();
        artists
            .iter()
            .take(2)
            .flat_map(|artist| {
                let mut followups = vec![(artist.name.clone(), 40)];
                if !query_tail.is_empty() {
                    followups.push((format!("{} {}", artist.name, query_tail), 46));
                }
                followups
            })
            .collect()
    }

    async fn collect_catalog_tracks(
        &self,
        search_query: &str,
        catalog: TidalSearchCatalog,
        limit_per_query: usize,
    ) -> Result<Vec<DiscoveryCandidateTrack>> {
        let mut tracks = catalog
            .tracks
            .into_iter()
            .map(|track| Self::map_track(track, "track-search", 32))
            .collect::<Vec<_>>();

        for album in catalog.albums.into_iter().take(2) {
            let album_tracks = self.client.get_album_tracks(album.id).await?;
            tracks.extend(album_tracks.items.into_iter().take(3).map(|track| {
                let search_track = TidalSearchTrack {
                    id: track.id,
                    title: track.title,
                    duration: track.duration,
                    artist_id: Some(track.artist.id),
                    artist_name: track
                        .artists
                        .as_ref()
                        .and_then(|artists| artists.first())
                        .map(|artist| artist.name.clone())
                        .or_else(|| Some(track.artist.name.clone())),
                    album_title: track.album.as_ref().map(|album| album.title.clone()),
                    artwork_url: TidalClient::get_artwork_url(
                        &track.album.as_ref().and_then(|album| album.cover.clone()),
                        640,
                    ),
                    audio_quality: track.audio_quality.clone(),
                    stream_ready: track.stream_ready,
                    extra: track.extra.clone(),
                };
                Self::map_track(search_track, "album-seed", 58)
            }));
        }

        let mut seen_followups = HashSet::new();
        for (followup_query, strength) in Self::artist_followups(search_query, &catalog.artists) {
            if !seen_followups.insert(followup_query.to_ascii_lowercase()) {
                continue;
            }
            let followup_tracks = self
                .client
                .search(&followup_query, limit_per_query as i32)
                .await?;
            tracks.extend(
                followup_tracks
                    .into_iter()
                    .take(3)
                    .map(|track| Self::map_track(track, "artist-seed", strength)),
            );
        }

        Ok(tracks)
    }
}

#[async_trait]
impl DiscoveryProvider for TidalDiscoveryProvider {
    fn capabilities(&self) -> DiscoveryProviderCapability {
        DiscoveryProviderCapability {
            provider: "tidal".to_string(),
            can_save: true,
            can_play_inline: true,
            can_fetch_connections: true,
            can_map_genres: true,
        }
    }

    async fn search_tracks(
        &self,
        queries: &[String],
        limit_per_query: usize,
    ) -> Result<Vec<DiscoveryCandidateTrack>> {
        let mut tracks = Vec::new();
        for query in queries {
            let catalog = self
                .client
                .search_catalog(query, limit_per_query as i32)
                .await?;
            tracks.extend(
                self.collect_catalog_tracks(query, catalog, limit_per_query)
                    .await?,
            );
        }
        Ok(tracks)
    }

    async fn connected_tracks(
        &self,
        seed: &DiscoveryCandidateSeed,
        queries: &[String],
        limit_per_query: usize,
    ) -> Result<Vec<DiscoveryCandidateTrack>> {
        let mut tracks = Vec::new();

        if let Some(album_title) = seed.album_title.as_deref() {
            let album_query = format!(
                "{} {}",
                seed.artist_name.as_deref().unwrap_or_default(),
                album_title
            );
            let catalog = self.client.search_catalog(&album_query, 4).await?;
            tracks.extend(
                self.collect_catalog_tracks(&album_query, catalog, limit_per_query)
                    .await?
                    .into_iter()
                    .map(|mut track| {
                        track.seed_kind = "connected-album-seed".to_string();
                        track.seed_strength = track.seed_strength.max(64);
                        track
                    }),
            );
        }

        for query in queries {
            let catalog = self
                .client
                .search_catalog(query, limit_per_query as i32)
                .await?;
            tracks.extend(
                self.collect_catalog_tracks(query, catalog, limit_per_query)
                    .await?
                    .into_iter()
                    .map(|mut track| {
                        if track.seed_kind == "artist-seed" || track.seed_kind == "album-seed" {
                            track.seed_strength = track.seed_strength.max(60);
                        }
                        track
                    }),
            );
        }

        Ok(tracks)
    }

    async fn save_track(&self, provider_track_id: &str) -> Result<()> {
        let track_id = provider_track_id.parse::<i64>()?;
        tidal_mutations::add_favorite_track(
            &self.http,
            &self.access_token,
            &self.user_id,
            track_id,
            &self.country_code,
        )
        .await
    }
}

fn collect_tidal_genre_hints(track: &TidalSearchTrack) -> Vec<String> {
    let mut hints = Vec::new();
    for (key, value) in &track.extra {
        let key = key.to_ascii_lowercase();
        if !key.contains("genre") && !key.contains("mood") && !key.contains("tag") {
            continue;
        }
        collect_json_strings(value, &mut hints);
    }
    hints.extend(infer_genres_from_text(&track.title));
    if let Some(artist_name) = track.artist_name.as_deref() {
        hints.extend(infer_genres_from_text(artist_name));
    }
    if let Some(album_title) = track.album_title.as_deref() {
        hints.extend(infer_genres_from_text(album_title));
    }
    hints.sort();
    hints.dedup();
    hints
}

fn collect_metadata_tokens(track: &TidalSearchTrack, raw_genre_hints: &[String]) -> Vec<String> {
    let mut tokens = Vec::new();
    tokens.extend(tokenize(&track.title));
    if let Some(artist_name) = track.artist_name.as_deref() {
        tokens.extend(tokenize(artist_name));
    }
    if let Some(album_title) = track.album_title.as_deref() {
        tokens.extend(tokenize(album_title));
    }
    for hint in raw_genre_hints {
        tokens.extend(tokenize(hint));
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

fn collect_json_strings(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_strings(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for value in map.values() {
                collect_json_strings(value, out);
            }
        }
        _ => {}
    }
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .to_ascii_lowercase()
        .split(|char: char| !char.is_ascii_alphanumeric())
        .filter(|part| !part.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn infer_genres_from_text(value: &str) -> Vec<String> {
    let builder = embedded_builder();
    let words = tokenize(value);
    let mut normalized = HashSet::new();
    for start in 0..words.len() {
        for width in 1..=3 {
            if start + width > words.len() {
                break;
            }
            let phrase = words[start..start + width].join(" ");
            if let Some(canonical) = builder.normalize(&phrase) {
                normalized.insert(canonical);
            }
        }
    }
    let mut normalized = normalized.into_iter().collect::<Vec<_>>();
    normalized.sort();
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn collects_genre_hints_from_album_metadata_titles() {
        let track = TidalSearchTrack {
            id: 1,
            title: "Limbus".to_string(),
            duration: 449,
            artist_id: None,
            artist_name: Some("Nadja Lind".to_string()),
            album_title: Some(
                "Deep Space Night - Panorama of Dub Techno, Minimal Deep Berlin Underground Club Tech House & Dreamy Chill out Music".to_string(),
            ),
            artwork_url: None,
            audio_quality: Some("LOSSLESS".to_string()),
            stream_ready: Some(true),
            extra: HashMap::new(),
        };

        let hints = collect_tidal_genre_hints(&track);

        assert!(hints.iter().any(|hint| hint == "Dub Techno"));
        assert!(hints.iter().any(|hint| hint == "Tech House"));
    }
}
